use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig, StreamError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

// length of one analysis frame
const FRAME: Duration = Duration::from_millis(5);
// time to learn the room before listening
const WARMUP: Duration = Duration::from_millis(500);
// keep ignoring sensors this long after playback stops
pub const PLAYBACK_TAIL: Duration = Duration::from_millis(250);
// noise floor rises slowly and falls fast
const FLOOR_RISE_SECS: f32 = 3.0;
const FLOOR_FALL_SECS: f32 = 0.3;
// short term loudness used to spot sudden jumps
const ONSET_SECS: f32 = 0.04;
// cutoff for the dc blocker in hz
const DC_CUTOFF_HZ: f32 = 30.0;
// keeps ratios sane on mics that gate silence to zero
const MIN_FLOOR: f32 = 1e-5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitSource {
    Mic,
    Accel,
}

impl HitSource {
    pub fn label(self) -> &'static str {
        match self {
            HitSource::Mic => "mic",
            HitSource::Accel => "accel",
        }
    }
}

pub struct Hit {
    pub source: HitSource,
    // raw size of the hit in full scale or g
    pub peak: f32,
    // zero at the trigger point and one for the hardest hits
    pub strength: f32,
    // absolute power from 0 to 100 for stats
    pub power: u8,
}

// what one sensitivity value means for each sensor
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub min_peak: f32,
    pub snr: f32,
    pub onset: f32,
    pub accel_g: f32,
}

// trigger level at sensitivity 1 and how much each step lowers it
const TOP_DB: f32 = -6.0;
const STEP_DB: f32 = 6.0;

impl Thresholds {
    // sensitivity 1 needs hard hits and 10 hears taps on very quiet mics
    pub fn new(sensitivity: f32) -> Self {
        let sensitivity = sensitivity.clamp(1.0, 10.0);
        let s = (sensitivity - 1.0) / 9.0;
        Self {
            // each step doubles sensitivity, from minus 6 db down to minus 60 db
            min_peak: from_db(TOP_DB - (sensitivity - 1.0) * STEP_DB),
            // a hit has to stand 10 db over the room and jump suddenly
            snr: 3.0,
            onset: 3.0,
            accel_g: 0.5 * 0.1f32.powf(s),
        }
    }
}

pub fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn to_db(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
}

// sensitivity whose trigger sits at this level
pub fn sensitivity_for(trigger: f32) -> f32 {
    (1.0 + (TOP_DB - to_db(trigger)) / STEP_DB).clamp(1.0, 10.0)
}

// state shared with the main thread without locks
#[derive(Default)]
pub struct Shared {
    peak: AtomicU32,
    floor: AtomicU32,
    frames: AtomicU32,
    min_peak: AtomicU32,
    snr: AtomicU32,
    onset: AtomicU32,
    accel_g: AtomicU32,
    pub playing: AtomicBool,
    pub failed: AtomicBool,
}

fn load(value: &AtomicU32) -> f32 {
    f32::from_bits(value.load(Ordering::Relaxed))
}

fn store(value: &AtomicU32, x: f32) {
    value.store(x.to_bits(), Ordering::Relaxed);
}

impl Shared {
    pub fn new(sensitivity: f32) -> Self {
        let shared = Self::default();
        shared.set_sensitivity(sensitivity);
        shared
    }

    pub fn set_sensitivity(&self, sensitivity: f32) {
        let t = Thresholds::new(sensitivity);
        store(&self.min_peak, t.min_peak);
        store(&self.snr, t.snr);
        store(&self.onset, t.onset);
        store(&self.accel_g, t.accel_g);
    }

    // loudest peak since the last call
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }

    pub fn floor(&self) -> f32 {
        load(&self.floor)
    }

    pub fn min_peak(&self) -> f32 {
        load(&self.min_peak)
    }

    pub fn accel_g(&self) -> f32 {
        load(&self.accel_g)
    }

    // counts analysis frames so a stalled mic can be spotted
    pub fn frames(&self) -> u32 {
        self.frames.load(Ordering::Relaxed)
    }
}

pub fn input_names() -> anyhow::Result<Vec<String>> {
    Ok(cpal::default_host()
        .input_devices()?
        .filter_map(|d| d.name().ok())
        .collect())
}

// keeps the mic open until dropped
pub struct Mic {
    _stream: Stream,
    pub name: String,
}

// opens a mic by part of its name or the default one
pub fn start(
    wanted: Option<&str>,
    cooldown: Duration,
    tx: Sender<Hit>,
    shared: Arc<Shared>,
) -> anyhow::Result<Mic> {
    let host = cpal::default_host();
    let device = match wanted {
        Some(wanted) => {
            let needle = wanted.to_lowercase();
            host.input_devices()?
                .find(|d| d.name().is_ok_and(|n| n.to_lowercase().contains(&needle)))
                .with_context(|| format!("No microphone matching '{}'", wanted))?
        }
        None => host
            .default_input_device()
            .context("No input device available")?,
    };

    let config = device.default_input_config()?;
    let format = config.sample_format();
    let stream_config: StreamConfig = config.into();

    let name = format!(
        "{} ({:?}, {}ch @ {} Hz)",
        device.name().unwrap_or_else(|_| "unknown".to_string()),
        format,
        stream_config.channels,
        stream_config.sample_rate.0
    );

    shared.failed.store(false, Ordering::Relaxed);
    let detector = Detector::new(&stream_config, cooldown, tx, shared.clone());

    let stream = match format {
        SampleFormat::I8 => build::<i8>(&device, &stream_config, detector, shared)?,
        SampleFormat::I16 => build::<i16>(&device, &stream_config, detector, shared)?,
        SampleFormat::I32 => build::<i32>(&device, &stream_config, detector, shared)?,
        SampleFormat::I64 => build::<i64>(&device, &stream_config, detector, shared)?,
        SampleFormat::U8 => build::<u8>(&device, &stream_config, detector, shared)?,
        SampleFormat::U16 => build::<u16>(&device, &stream_config, detector, shared)?,
        SampleFormat::U32 => build::<u32>(&device, &stream_config, detector, shared)?,
        SampleFormat::U64 => build::<u64>(&device, &stream_config, detector, shared)?,
        SampleFormat::F32 => build::<f32>(&device, &stream_config, detector, shared)?,
        SampleFormat::F64 => build::<f64>(&device, &stream_config, detector, shared)?,
        other => anyhow::bail!("Unsupported input sample format: {:?}", other),
    };

    stream.play()?;
    Ok(Mic {
        _stream: stream,
        name,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut detector: Detector,
    shared: Arc<Shared>,
) -> anyhow::Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            for frame in data.chunks_exact(channels) {
                detector.push_frame(frame.iter().map(|s| s.to_sample::<f32>()));
            }
        },
        move |err| {
            // glitches are survivable but an unplugged mic is not
            if matches!(err, StreamError::DeviceNotAvailable) {
                shared.failed.store(true, Ordering::Relaxed);
            }
        },
        None,
    )?;

    Ok(stream)
}

// transient detector that runs inside the audio callback
struct Detector {
    tx: Sender<Hit>,
    shared: Arc<Shared>,

    frame_len: usize,
    count: usize,
    sum_sq: f32,
    peak: f32,

    dc_coef: f32,
    // last input and output of the dc blocker for each channel
    dc: Vec<(f32, f32)>,

    floor: f32,
    recent: f32,
    lagged: f32,
    // rms of the last two frames, newest first
    prev_rms: [f32; 2],
    floor_rise: f32,
    floor_fall: f32,
    onset_alpha: f32,

    cooldown_frames: u32,
    cooldown_left: u32,
    tail_frames: u32,
    mute_left: u32,
    warmup_left: u32,
}

impl Detector {
    fn new(
        config: &StreamConfig,
        cooldown: Duration,
        tx: Sender<Hit>,
        shared: Arc<Shared>,
    ) -> Self {
        let rate = config.sample_rate.0.max(1) as f32;
        let frame_len = ((rate * FRAME.as_secs_f32()) as usize).max(1);
        let frame_secs = frame_len as f32 / rate;

        let frames = |d: Duration| (d.as_secs_f32() / frame_secs).ceil() as u32;
        // one pole smoothing factor for a time constant
        let alpha = |tau: f32| 1.0 - (-frame_secs / tau).exp();

        Self {
            tx,
            shared,

            frame_len,
            count: 0,
            sum_sq: 0.0,
            peak: 0.0,

            dc_coef: (-2.0 * std::f32::consts::PI * DC_CUTOFF_HZ / rate).exp(),
            dc: vec![(0.0, 0.0); config.channels.max(1) as usize],

            floor: 0.0,
            recent: 0.0,
            lagged: 0.0,
            prev_rms: [0.0; 2],
            floor_rise: alpha(FLOOR_RISE_SECS),
            floor_fall: alpha(FLOOR_FALL_SECS),
            onset_alpha: alpha(ONSET_SECS),

            cooldown_frames: frames(cooldown),
            cooldown_left: 0,
            tail_frames: frames(PLAYBACK_TAIL),
            mute_left: 0,
            warmup_left: frames(WARMUP),
        }
    }

    // channels are combined by energy so out of phase mics never cancel
    #[inline]
    fn push_frame(&mut self, frame: impl Iterator<Item = f32>) {
        let mut energy = 0.0;
        let mut used = 0;
        for (x, (prev_in, prev_out)) in frame.zip(self.dc.iter_mut()) {
            // dc blocker strips offset and low rumble
            let y = x - *prev_in + self.dc_coef * *prev_out;
            *prev_in = x;
            *prev_out = y;
            energy += y * y;
            self.peak = self.peak.max(y.abs());
            used += 1;
        }

        self.sum_sq += energy / used.max(1) as f32;
        self.count += 1;

        if self.count == self.frame_len {
            self.end_frame();
        }
    }

    // compares against both the smoothed level and the real level 10 ms ago
    // so slow swells like voices never look like a sudden jump
    fn onset_reference(&self) -> f32 {
        self.lagged.max(self.prev_rms[1]).max(MIN_FLOOR)
    }

    fn end_frame(&mut self) {
        let rms = (self.sum_sq / self.count as f32).sqrt();
        let peak = self.peak;
        self.count = 0;
        self.sum_sq = 0.0;
        self.peak = 0.0;

        let shared = &self.shared;
        shared.frames.fetch_add(1, Ordering::Relaxed);
        // positive floats keep their order as raw bits
        shared.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);

        // stay deaf while our own sound plays so it cannot retrigger
        let playing = shared.playing.load(Ordering::Relaxed);
        if playing {
            self.mute_left = self.tail_frames;
        }

        let warming = self.warmup_left > 0;
        let listening = !warming && self.mute_left == 0 && self.cooldown_left == 0;
        self.warmup_left = self.warmup_left.saturating_sub(1);
        self.mute_left = self.mute_left.saturating_sub(1);
        self.cooldown_left = self.cooldown_left.saturating_sub(1);

        let floor = self.floor.max(MIN_FLOOR);
        let min_peak = load(&shared.min_peak);

        // a hit is loud, well above the room, and a sudden jump
        let hit = listening
            && peak >= min_peak
            && rms >= floor * load(&shared.snr)
            && rms >= self.onset_reference() * load(&shared.onset);

        if hit {
            self.cooldown_left = self.cooldown_frames;
            let _ = self.tx.send(Hit {
                source: HitSource::Mic,
                peak,
                strength: ((peak / min_peak).ln() / (1.0 / min_peak).ln()).clamp(0.0, 1.0),
                // minus 60 db maps to 0 and full scale to 100
                power: ((to_db(peak) + 60.0) / 0.6).clamp(0.0, 100.0) as u8,
            });
        }

        // floor tracks the background and skips hits and playback
        if warming {
            self.floor += (rms - self.floor) * self.onset_alpha;
        } else if !playing && !hit && self.cooldown_left == 0 {
            let a = if rms > self.floor {
                self.floor_rise
            } else {
                self.floor_fall
            };
            self.floor += (rms - self.floor) * a;
        }
        store(&shared.floor, self.floor);

        // reference lags a frame so hits split across frames still count
        self.lagged = self.recent;
        self.prev_rms = [rms, self.prev_rms[0]];
        self.recent += (rms - self.recent) * self.onset_alpha;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const RATE: u32 = 48_000;

    fn detector(sensitivity: f32) -> (Detector, mpsc::Receiver<Hit>, Arc<Shared>) {
        let config = StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::new(sensitivity));
        (
            Detector::new(&config, Duration::from_millis(450), tx, shared.clone()),
            rx,
            shared,
        )
    }

    // cheap repeatable noise in the range of a quiet room
    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32 - 0.5) * 0.01
    }

    // feeds secs of audio where extra adds signal at time t
    fn feed(d: &mut Detector, seed: &mut u32, secs: f32, extra: impl Fn(f32) -> f32) {
        for i in 0..(secs * RATE as f32) as usize {
            let t = i as f32 / RATE as f32;
            d.push_frame(std::iter::once(noise(seed) + extra(t)));
        }
    }

    // decaying 200 hz thump like a hand on the chassis
    fn knock(t: f32, amp: f32) -> f32 {
        amp * (-t / 0.01).exp() * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
    }

    #[test]
    fn detects_knock() {
        let (mut d, rx, _) = detector(5.0);
        let mut seed = 1;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.2, |t| knock(t, 0.6));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn ignores_quiet_room() {
        let (mut d, rx, _) = detector(10.0);
        let mut seed = 7;
        feed(&mut d, &mut seed, 3.0, |_| 0.0);
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn ignores_slow_loud_sound() {
        let (mut d, rx, _) = detector(5.0);
        let mut seed = 3;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        // tone that swells over 300 ms like a voice or music
        feed(&mut d, &mut seed, 1.0, |t| {
            0.5 * (t / 0.3).min(1.0) * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
        });
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn ignores_light_tap_at_default() {
        let (mut d, rx, _) = detector(5.0);
        let mut seed = 5;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.2, |t| knock(t, 0.012));
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn high_sensitivity_hears_light_tap() {
        let (mut d, rx, _) = detector(10.0);
        let mut seed = 13;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.2, |t| knock(t, 0.06));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn max_sensitivity_hears_gated_quiet_mic() {
        // noise suppressed mics output pure zeros and squash hits to about minus 45 db
        let config = StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(RATE),
            buffer_size: cpal::BufferSize::Default,
        };
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::new(10.0));
        let mut d = Detector::new(&config, Duration::from_millis(450), tx, shared);
        for i in 0..RATE as usize * 2 {
            let t = i as f32 / RATE as f32;
            let x = if t > 1.0 { knock(t - 1.0, 0.006) } else { 0.0 };
            // second channel is wired backwards which used to cancel out
            d.push_frame([x, -x].into_iter());
        }
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn runtime_sensitivity_change_applies() {
        let (mut d, rx, shared) = detector(1.0);
        let mut seed = 15;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.6, |t| knock(t, 0.1));
        assert_eq!(rx.try_iter().count(), 0);
        shared.set_sensitivity(10.0);
        feed(&mut d, &mut seed, 0.6, |t| knock(t, 0.1));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn ignores_own_playback() {
        let (mut d, rx, shared) = detector(5.0);
        let mut seed = 9;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        shared.playing.store(true, Ordering::Relaxed);
        feed(&mut d, &mut seed, 0.2, |t| knock(t, 0.6));
        shared.playing.store(false, Ordering::Relaxed);
        feed(&mut d, &mut seed, 0.1, |_| 0.0);
        assert_eq!(rx.try_iter().count(), 0);
        // listens again once the tail has passed
        feed(&mut d, &mut seed, 0.5, |_| 0.0);
        feed(&mut d, &mut seed, 0.2, |t| knock(t, 0.6));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn cooldown_merges_double_hits() {
        let (mut d, rx, _) = detector(5.0);
        let mut seed = 11;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.3, |t| {
            knock(t, 0.6) + knock((t - 0.1).max(0.0), 0.6)
        });
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn harder_hits_are_stronger() {
        let (mut d, rx, _) = detector(5.0);
        let mut seed = 17;
        feed(&mut d, &mut seed, 1.0, |_| 0.0);
        feed(&mut d, &mut seed, 0.6, |t| knock(t, 0.15));
        feed(&mut d, &mut seed, 0.6, |t| knock(t, 0.9));
        let hits: Vec<Hit> = rx.try_iter().collect();
        assert_eq!(hits.len(), 2);
        assert!(hits[1].strength > hits[0].strength);
        assert!(hits[1].power > hits[0].power);
    }

    #[test]
    fn sensitivity_for_round_trips() {
        for db in [-10.0, -30.0, -45.0, -59.0] {
            let level = from_db(db);
            let t = Thresholds::new(sensitivity_for(level));
            assert!(
                (to_db(t.min_peak) - db).abs() < 0.01,
                "{} came back as {}",
                db,
                to_db(t.min_peak)
            );
        }
        assert_eq!(sensitivity_for(from_db(-90.0)), 10.0);
        assert_eq!(sensitivity_for(1.0), 1.0);
    }
}
