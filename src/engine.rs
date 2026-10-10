use crate::{
    accel::{self, Accel},
    audio::{BUILT_IN, Pack, Player},
    config::{Config, SourceMode},
    detector::{self, Hit, HitSource, Mic, Shared},
    stats::Stats,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

// hits this close after the last sound keep the combo going
const COMBO_WINDOW: Duration = Duration::from_millis(1500);
// combos speed the sound up to this much
const COMBO_MAX_SPEED: f32 = 1.5;
// no audio for this long means the mic died quietly
const MIC_STALL: Duration = Duration::from_secs(3);
const MIC_RETRY: Duration = Duration::from_secs(3);
const CAL_QUIET: Duration = Duration::from_millis(1500);
// longest calibration listens for hits
const CAL_LISTEN: Duration = Duration::from_secs(8);
// ends early this long after the last hit once enough were heard
const CAL_SETTLE: Duration = Duration::from_millis(1200);
const CAL_DONE_SHOW: Duration = Duration::from_secs(6);
pub const CAL_TAPS: usize = 3;
// how long one sound is measured and the gap before the next
const CAL_EVENT_LEN: Duration = Duration::from_millis(120);
const CAL_EVENT_GAP: Duration = Duration::from_millis(250);
const LOG_LEN: usize = 200;
const SENSITIVITY_STEP: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    TogglePause,
    NextPack,
    SensitivityUp,
    SensitivityDown,
    Calibrate,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Warn,
    Hit,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub seq: u64,
    pub time: String,
    pub kind: LogKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Listening,
    Paused,
    Calibrating,
    NoInput,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalibrationView {
    Quiet {
        progress: f32,
    },
    Listen {
        progress: f32,
        // loudest sounds heard so far, loudest first
        best: Vec<f32>,
    },
    Done {
        taps: Vec<f32>,
        trigger: f32,
        sensitivity: f32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct LastHit {
    pub age: Duration,
    pub power: u8,
    pub source: HitSource,
}

// everything a front end needs to draw one frame
#[derive(Clone, Debug)]
pub struct View {
    pub pack: String,
    pub pack_len: usize,
    pub status: Status,
    pub sensitivity: f32,
    pub cooldown_ms: u64,
    pub mic: Option<String>,
    pub accel: Option<String>,
    pub threshold: f32,
    pub combo: u32,
    pub last_hit: Option<LastHit>,
    pub calibration: Option<CalibrationView>,
    pub stats: Stats,
    pub log: Vec<LogLine>,
}

// works on raw mic peaks so it can find hits the detector would miss
enum Calibration {
    Quiet {
        started: Instant,
        loudest: f32,
    },
    Listen {
        noise: f32,
        started: Instant,
        events: Vec<f32>,
        current: Option<(Instant, f32)>,
        ready_at: Instant,
        last_event: Option<Instant>,
    },
    // result stays on screen for a moment
    Done {
        until: Instant,
        taps: Vec<f32>,
        trigger: f32,
        sensitivity: f32,
    },
}

pub struct Engine {
    pub config: Config,
    stats: Stats,
    player: Player,
    pack: Pack,
    shared: Arc<Shared>,
    tx: Sender<Hit>,
    rx: Receiver<Hit>,

    mic: Option<Mic>,
    mic_retry_at: Instant,
    mic_lost: bool,
    last_frames: u32,
    last_frames_at: Instant,
    accel: Option<Accel>,

    paused: bool,
    combo: u32,
    last_hit_at: Option<Instant>,
    last_hit: Option<(Instant, u8, HitSource)>,
    playback_end: Option<Instant>,
    was_playing: bool,
    calibration: Option<Calibration>,
    level: f32,

    log: VecDeque<LogLine>,
    log_seq: u64,
    quit: bool,
}

impl Engine {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let pack = Pack::load(&config.mode, config.custom_folder.as_deref())?;
        let player = Player::new()?;
        let stats = Stats::load().unwrap_or_default();
        let shared = Arc::new(Shared::new(config.sensitivity));
        let (tx, rx) = mpsc::channel();
        let now = Instant::now();

        let mut engine = Self {
            config,
            stats,
            player,
            pack,
            shared,
            tx,
            rx,
            mic: None,
            mic_retry_at: now,
            mic_lost: false,
            last_frames: 0,
            last_frames_at: now,
            accel: None,
            paused: false,
            combo: 0,
            last_hit_at: None,
            last_hit: None,
            playback_end: None,
            was_playing: false,
            calibration: None,
            level: 0.0,
            log: VecDeque::new(),
            log_seq: 0,
            quit: false,
        };

        engine.log_pack();

        let source = engine.config.source;
        if source != SourceMode::Mic {
            engine.accel =
                accel::start(engine.cooldown(), engine.tx.clone(), engine.shared.clone());
            match &engine.accel {
                Some(accel) => engine.log(
                    LogKind::Info,
                    format!("Accelerometer ready: {}", accel.name),
                ),
                None if source == SourceMode::Accel => {
                    anyhow::bail!("No accelerometer found on this machine")
                }
                None => engine.log(LogKind::Info, "No accelerometer found, using the mic"),
            }
        }

        if source != SourceMode::Accel
            && let Err(err) = engine.open_mic()
        {
            // only fatal when there is nothing else to listen with
            if engine.accel.is_none() {
                return Err(err);
            }
            engine.log(LogKind::Warn, format!("{:#}", err));
            engine.mic_lost = true;
        }

        Ok(engine)
    }

    // call this often from the front end loop
    pub fn tick(&mut self) {
        let now = Instant::now();

        let playing = self.player.is_playing();
        if self.was_playing && !playing {
            self.playback_end = Some(now);
        }
        self.was_playing = playing;
        self.shared.playing.store(playing, Ordering::Relaxed);

        // the engine is the only reader of mic peaks
        let peak = self.shared.take_peak();
        self.level = self.level.max(peak);

        while let Ok(hit) = self.rx.try_recv() {
            self.on_hit(hit, now);
        }

        self.check_mic(now);
        self.check_calibration(now, peak);
    }

    pub fn command(&mut self, command: Command) {
        match command {
            Command::TogglePause => {
                self.paused = !self.paused;
                let text = if self.paused {
                    "Paused"
                } else {
                    "Listening again"
                };
                self.log(LogKind::Info, text);
            }
            Command::NextPack => self.next_pack(),
            Command::SensitivityUp => self.nudge_sensitivity(SENSITIVITY_STEP),
            Command::SensitivityDown => self.nudge_sensitivity(-SENSITIVITY_STEP),
            Command::Calibrate => self.start_calibration(),
            Command::Quit => {
                self.save_stats();
                self.quit = true;
            }
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    #[cfg(feature = "tray")]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    #[cfg(feature = "tray")]
    pub fn pack_name(&self) -> &str {
        &self.pack.name
    }

    #[cfg(feature = "tray")]
    pub fn hits_today(&self) -> u32 {
        self.stats.today()
    }

    // loudest mic peak since the last call
    pub fn take_level(&mut self) -> f32 {
        std::mem::take(&mut self.level)
    }

    pub fn floor(&self) -> f32 {
        self.shared.floor()
    }

    pub fn logs_after(&self, seq: u64) -> impl Iterator<Item = &LogLine> {
        self.log.iter().filter(move |line| line.seq > seq)
    }

    pub fn view(&self, log_lines: usize) -> View {
        let calibration = match &self.calibration {
            Some(Calibration::Quiet { started, .. }) => Some(CalibrationView::Quiet {
                progress: (started.elapsed().as_secs_f32() / CAL_QUIET.as_secs_f32()).min(1.0),
            }),
            Some(Calibration::Listen {
                started, events, ..
            }) => {
                let mut best = events.clone();
                best.sort_by(|a, b| b.total_cmp(a));
                best.truncate(CAL_TAPS);
                Some(CalibrationView::Listen {
                    progress: (started.elapsed().as_secs_f32() / CAL_LISTEN.as_secs_f32()).min(1.0),
                    best,
                })
            }
            Some(Calibration::Done {
                taps,
                trigger,
                sensitivity,
                ..
            }) => Some(CalibrationView::Done {
                taps: taps.clone(),
                trigger: *trigger,
                sensitivity: *sensitivity,
            }),
            None => None,
        };
        let status = if self.calibrating() {
            Status::Calibrating
        } else if self.mic.is_none() && self.accel.is_none() {
            Status::NoInput
        } else if self.paused {
            Status::Paused
        } else {
            Status::Listening
        };

        View {
            pack: self.pack.name.clone(),
            pack_len: self.pack.len(),
            status,
            sensitivity: self.config.sensitivity,
            cooldown_ms: self.config.cooldown_ms,
            mic: self.mic.as_ref().map(|m| m.name.clone()),
            accel: self.accel.as_ref().map(|a| a.name.clone()),
            threshold: self.shared.min_peak(),
            combo: self.live_combo(),
            last_hit: self.last_hit.map(|(at, power, source)| LastHit {
                age: at.elapsed(),
                power,
                source,
            }),
            calibration,
            stats: self.stats.clone(),
            log: self
                .log
                .iter()
                .rev()
                .take(log_lines)
                .rev()
                .cloned()
                .collect(),
        }
    }

    // the combo only counts while another hit could still extend it
    fn live_combo(&self) -> u32 {
        let since = self.playback_end.max(self.last_hit_at);
        let live = self.was_playing || since.is_some_and(|t| t.elapsed() <= COMBO_WINDOW);
        if live { self.combo } else { 0 }
    }

    fn cooldown(&self) -> Duration {
        Duration::from_millis(self.config.cooldown_ms)
    }

    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.log_seq += 1;
        self.log.push_back(LogLine {
            seq: self.log_seq,
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            kind,
            text: text.into(),
        });
        if self.log.len() > LOG_LEN {
            self.log.pop_front();
        }
    }

    fn log_pack(&mut self) {
        let text = format!(
            "Loaded {} pack ({} sounds)",
            self.pack.name,
            self.pack.len()
        );
        self.log(LogKind::Info, text);
        for line in std::mem::take(&mut self.pack.skipped) {
            self.log(LogKind::Warn, line);
        }
    }

    fn save_config(&mut self) {
        if let Err(err) = self.config.save() {
            self.log(LogKind::Warn, format!("Could not save config: {:#}", err));
        }
    }

    fn save_stats(&mut self) {
        if let Err(err) = self.stats.save() {
            self.log(LogKind::Warn, format!("Could not save stats: {:#}", err));
        }
    }

    fn open_mic(&mut self) -> anyhow::Result<()> {
        let wanted = self.config.mic.clone();
        let (cooldown, tx, shared) = (self.cooldown(), self.tx.clone(), self.shared.clone());
        let start =
            |wanted: Option<&str>| detector::start(wanted, cooldown, tx.clone(), shared.clone());

        let mic = match start(wanted.as_deref()) {
            Ok(mic) => mic,
            Err(err) if wanted.is_some() => {
                let fallback = start(None)?;
                self.log(LogKind::Warn, format!("{:#}, using the default mic", err));
                fallback
            }
            Err(err) => return Err(err),
        };

        self.log(LogKind::Info, format!("Mic ready: {}", mic.name));
        self.mic = Some(mic);
        self.mic_lost = false;
        self.last_frames = self.shared.frames();
        self.last_frames_at = Instant::now();
        Ok(())
    }

    // reopens the mic after it is unplugged or stops sending audio
    fn check_mic(&mut self, now: Instant) {
        if self.config.source == SourceMode::Accel {
            return;
        }

        if self.mic.is_some() {
            let frames = self.shared.frames();
            if frames != self.last_frames {
                self.last_frames = frames;
                self.last_frames_at = now;
            }

            let failed = self.shared.failed.swap(false, Ordering::Relaxed);
            if failed || now.duration_since(self.last_frames_at) > MIC_STALL {
                self.mic = None;
                self.mic_lost = true;
                self.mic_retry_at = now + Duration::from_secs(1);
                self.log(LogKind::Warn, "Microphone lost, trying to reconnect");
            }
        } else if now >= self.mic_retry_at {
            self.mic_retry_at = now + MIC_RETRY;
            if let Err(err) = self.open_mic() {
                // only report the first failure of an outage
                if !self.mic_lost {
                    self.log(LogKind::Warn, format!("{:#}", err));
                }
                self.mic_lost = true;
            }
        }
    }

    fn on_hit(&mut self, hit: Hit, now: Instant) {
        if self.calibrating() {
            return;
        }
        if self.paused || self.player.is_playing() {
            return;
        }
        // mic and accelerometer often both catch the same slap
        if self
            .last_hit_at
            .is_some_and(|t| now.duration_since(t) < self.cooldown())
        {
            return;
        }

        let since = self.playback_end.max(self.last_hit_at);
        let chained = since.is_some_and(|t| now.duration_since(t) <= COMBO_WINDOW);
        self.combo = if self.config.combo && chained {
            self.combo + 1
        } else {
            1
        };
        self.last_hit_at = Some(now);
        self.last_hit = Some((now, hit.power, hit.source));

        let volume = if self.config.dynamic_volume {
            self.config.volume * (0.35 + 0.65 * hit.strength)
        } else {
            self.config.volume
        };
        let speed = (1.0 + 0.05 * (self.combo - 1) as f32).min(COMBO_MAX_SPEED);

        match self.player.play(&mut self.pack, volume, speed) {
            Ok(()) => {
                self.shared.playing.store(true, Ordering::Relaxed);
                self.was_playing = true;
            }
            Err(err) => self.log(LogKind::Warn, format!("Could not play sound: {:#}", err)),
        }

        let record = self.stats.record(hit.power, self.combo);
        self.save_stats();

        let mut text = match hit.source {
            HitSource::Mic => format!(
                "mic hit at {:.0} dB, power {}",
                detector::to_db(hit.peak),
                hit.power
            ),
            HitSource::Accel => format!("accel hit at {:.2} g, power {}", hit.peak, hit.power),
        };
        if self.combo > 1 {
            text.push_str(&format!(", combo x{}", self.combo));
        }
        if record {
            text.push_str(", new record");
        }
        self.log(LogKind::Hit, text);
    }

    fn next_pack(&mut self) {
        let mut modes: Vec<&str> = BUILT_IN.to_vec();
        if self.config.custom_folder.is_some() {
            modes.push("custom");
        }
        let index = modes
            .iter()
            .position(|m| *m == self.config.mode)
            .unwrap_or(0);
        let next = modes[(index + 1) % modes.len()].to_string();

        match Pack::load(&next, self.config.custom_folder.as_deref()) {
            Ok(pack) => {
                self.pack = pack;
                self.config.mode = next;
                self.log_pack();
                self.save_config();
            }
            Err(err) => self.log(LogKind::Warn, format!("{:#}", err)),
        }
    }

    fn nudge_sensitivity(&mut self, step: f32) {
        if self.calibrating() {
            return;
        }
        let value = ((self.config.sensitivity + step) * 2.0).round() / 2.0;
        self.set_sensitivity(value.clamp(1.0, 10.0));
        self.save_config();
    }

    fn set_sensitivity(&mut self, value: f32) {
        self.config.sensitivity = value;
        self.shared.set_sensitivity(value);
        self.log(LogKind::Info, format!("Sensitivity {:.1}", value));
    }

    fn calibrating(&self) -> bool {
        matches!(
            self.calibration,
            Some(Calibration::Quiet { .. } | Calibration::Listen { .. })
        )
    }

    fn start_calibration(&mut self) {
        if self.calibrating() {
            self.cancel_calibration("Calibration cancelled");
            return;
        }
        if self.mic.is_none() {
            self.log(LogKind::Warn, "Calibration needs a working mic");
            return;
        }
        self.calibration = Some(Calibration::Quiet {
            started: Instant::now(),
            loudest: 0.0,
        });
        self.log(LogKind::Info, "Calibrating, stay quiet for a moment");
    }

    // steps calibration with the loudest raw peak since the last tick
    fn check_calibration(&mut self, now: Instant, peak: f32) {
        let mut message = None;
        let mut result = None;

        match &mut self.calibration {
            None => return,
            Some(Calibration::Done { until, .. }) => {
                if now >= *until {
                    self.calibration = None;
                }
                return;
            }
            Some(Calibration::Quiet { started, loudest }) => {
                *loudest = loudest.max(peak);
                if now.duration_since(*started) >= CAL_QUIET {
                    self.calibration = Some(Calibration::Listen {
                        noise: *loudest,
                        started: now,
                        events: Vec::new(),
                        current: None,
                        ready_at: now,
                        last_event: None,
                    });
                    message = Some(format!(
                        "Now hit the laptop {} times, like you normally would",
                        CAL_TAPS
                    ));
                }
            }
            Some(Calibration::Listen {
                noise,
                started,
                events,
                current,
                ready_at,
                last_event,
            }) => {
                // anything clearly above the quiet room counts as a candidate
                let candidate = (*noise * 4.0).max(detector::from_db(-66.0));

                if let Some((start, loudest)) = current {
                    *loudest = loudest.max(peak);
                    if now.duration_since(*start) >= CAL_EVENT_LEN {
                        events.push(*loudest);
                        *current = None;
                        *ready_at = now + CAL_EVENT_GAP;
                        *last_event = Some(now);
                    }
                } else if now >= *ready_at && peak >= candidate {
                    *current = Some((now, peak));
                }

                // stop once the hits are in and things went quiet again
                let settled = events.len() >= CAL_TAPS
                    && last_event.is_some_and(|t| now.duration_since(t) >= CAL_SETTLE);
                let timed_out = now.duration_since(*started) >= CAL_LISTEN;

                if settled || (timed_out && !events.is_empty()) {
                    result = Some((events.clone(), *noise));
                } else if timed_out {
                    self.cancel_calibration("Calibration heard no hits, try hitting harder");
                    return;
                }
            }
        }

        if let Some(text) = message {
            self.log(LogKind::Info, text);
        }
        if let Some((events, noise)) = result {
            self.finish_calibration(events, noise, now);
        }
    }

    fn finish_calibration(&mut self, mut events: Vec<f32>, noise: f32, now: Instant) {
        // the loudest few are the real hits and anything softer was just noise
        events.sort_by(|a, b| b.total_cmp(a));
        events.truncate(CAL_TAPS);
        if events.len() < CAL_TAPS {
            self.log(
                LogKind::Warn,
                format!("Only heard {} of {} hits", events.len(), CAL_TAPS),
            );
        }

        let (trigger, crowded) = calibration_trigger(&events, noise);
        if crowded {
            self.log(
                LogKind::Warn,
                "Your hits are barely louder than the room, results may vary",
            );
        }

        let value = (detector::sensitivity_for(trigger) * 10.0).round() / 10.0;
        self.set_sensitivity(value);
        self.save_config();

        let trigger = self.shared.min_peak();
        let heard = events
            .iter()
            .map(|e| format!("{:.0}", detector::to_db(*e)))
            .collect::<Vec<_>>()
            .join(", ");
        self.log(
            LogKind::Info,
            format!(
                "Calibrated from hits at {} dB, sensitivity {:.1} triggers at {:.0} dB",
                heard,
                value,
                detector::to_db(trigger)
            ),
        );
        if detector::to_db(events[events.len() - 1]) < -54.0 {
            self.log(
                LogKind::Warn,
                "Your hits are very quiet, turning off mic audio enhancements should help",
            );
        }

        self.calibration = Some(Calibration::Done {
            until: now + CAL_DONE_SHOW,
            taps: events,
            trigger,
            sensitivity: value,
        });
    }

    fn cancel_calibration(&mut self, reason: &str) {
        self.calibration = None;
        self.log(LogKind::Warn, reason);
    }
}

// trigger for the loudest hits sorted loudest first, and whether the room crowded it
fn calibration_trigger(taps: &[f32], noise: f32) -> (f32, bool) {
    let strongest = taps[0];
    // one much softer hit should not make it trigger on everything
    let weakest = taps[taps.len() - 1].max(strongest / 8.0);

    // sit 6 db under the softest hit but stay 6 db above the room
    let room = noise * 2.0;
    let trigger = weakest * 0.5;
    if trigger < room {
        (room.min(weakest * 0.9), true)
    } else {
        (trigger, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::{from_db, to_db};

    // what finish_calibration does before picking a trigger
    fn loudest(mut events: Vec<f32>) -> Vec<f32> {
        events.sort_by(|a, b| b.total_cmp(a));
        events.truncate(CAL_TAPS);
        events
    }

    #[test]
    fn blips_do_not_drag_the_trigger_down() {
        // a gated mic with tiny blips mixed in with real hits
        let events = [-64.0, -30.0, -61.0, -28.0, -66.0, -33.0]
            .map(from_db)
            .to_vec();
        let taps = loudest(events);
        let (trigger, crowded) = calibration_trigger(&taps, 0.0);
        assert!(!crowded);
        assert!((to_db(trigger) - -39.0).abs() < 0.1, "{}", to_db(trigger));
        let s = detector::sensitivity_for(trigger);
        assert!(s > 6.0 && s < 7.0, "{}", s);
    }

    #[test]
    fn noisy_room_keeps_trigger_above_it() {
        let taps = loudest([-20.0, -22.0, -24.0].map(from_db).to_vec());
        let noise = from_db(-27.0);
        let (trigger, crowded) = calibration_trigger(&taps, noise);
        assert!(crowded);
        assert!(trigger >= noise && trigger < taps[2]);
    }

    #[test]
    fn one_soft_hit_is_capped() {
        let taps = loudest([-10.0, -12.0, -50.0].map(from_db).to_vec());
        let (trigger, _) = calibration_trigger(&taps, 0.0);
        // softest counted hit is 18 db under the strongest
        assert!((to_db(trigger) - -34.1).abs() < 0.2, "{}", to_db(trigger));
    }
}
