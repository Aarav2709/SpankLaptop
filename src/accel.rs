use crate::detector::{Hit, HitSource, PLAYBACK_TAIL, Shared};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};

// jolt in g that counts as a full power hit
const FULL_G: f32 = 2.0;

// stops the polling thread when dropped
pub struct Accel {
    stop: Arc<AtomicBool>,
    pub name: String,
}

impl Drop for Accel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// starts polling the accelerometer if this machine has one
pub fn start(cooldown: Duration, tx: Sender<Hit>, shared: Arc<Shared>) -> Option<Accel> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel();

    let stop_thread = stop.clone();
    let spawned = thread::Builder::new().name("accel".into()).spawn(move || {
        // the sensor is opened on its own thread so it never has to be sent
        let Some(sensor) = platform::open() else {
            let _ = ready_tx.send(None);
            return;
        };
        let _ = ready_tx.send(Some(sensor.name()));

        let mut detector = AccelDetector::new(cooldown, tx, shared);
        while !stop_thread.load(Ordering::Relaxed) {
            if let Some(reading) = sensor.read() {
                detector.push(reading, Instant::now());
            }
            thread::sleep(sensor.interval());
        }
    });

    let name = spawned
        .ok()
        .and_then(|_| ready_rx.recv_timeout(Duration::from_secs(3)).ok().flatten());

    match name {
        Some(name) => Some(Accel { stop, name }),
        None => {
            stop.store(true, Ordering::Relaxed);
            None
        }
    }
}

// a slap shows up as a sharp change between two readings
struct AccelDetector {
    tx: Sender<Hit>,
    shared: Arc<Shared>,
    cooldown: Duration,
    prev: Option<[f32; 3]>,
    last_hit: Option<Instant>,
    mute_until: Option<Instant>,
}

impl AccelDetector {
    fn new(cooldown: Duration, tx: Sender<Hit>, shared: Arc<Shared>) -> Self {
        Self {
            tx,
            shared,
            cooldown,
            prev: None,
            last_hit: None,
            mute_until: None,
        }
    }

    fn push(&mut self, reading: [f32; 3], now: Instant) {
        let Some(prev) = self.prev.replace(reading) else {
            return;
        };
        // polling faster than the sensor repeats the same reading
        if reading == prev {
            return;
        }

        // speakers shake the chassis too
        if self.shared.playing.load(Ordering::Relaxed) {
            self.mute_until = Some(now + PLAYBACK_TAIL);
            return;
        }
        if self.mute_until.is_some_and(|t| now < t) {
            return;
        }
        if self
            .last_hit
            .is_some_and(|t| now.duration_since(t) < self.cooldown)
        {
            return;
        }

        let jolt = reading
            .iter()
            .zip(prev)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt();

        let threshold = self.shared.accel_g();
        if jolt < threshold {
            return;
        }

        self.last_hit = Some(now);
        let _ = self.tx.send(Hit {
            source: HitSource::Accel,
            peak: jolt,
            strength: ((jolt / threshold).ln() / (FULL_G / threshold).ln()).clamp(0.0, 1.0),
            power: (jolt / FULL_G * 100.0).clamp(0.0, 100.0) as u8,
        });
    }
}

#[cfg(all(windows, feature = "accel"))]
mod platform {
    use std::time::Duration;
    use windows::Devices::Sensors::Accelerometer;

    pub struct Sensor {
        inner: Accelerometer,
        interval: Duration,
    }

    pub fn open() -> Option<Sensor> {
        let inner = Accelerometer::GetDefault().ok()?;
        let min = inner.MinimumReportInterval().unwrap_or(16).max(1);
        let _ = inner.SetReportInterval(min);
        let sensor = Sensor {
            inner,
            interval: Duration::from_millis(min.into()),
        };
        sensor.read()?;
        Some(sensor)
    }

    impl Sensor {
        pub fn name(&self) -> String {
            "Windows accelerometer".to_string()
        }

        pub fn interval(&self) -> Duration {
            self.interval
        }

        // readings are already in g
        pub fn read(&self) -> Option<[f32; 3]> {
            let r = self.inner.GetCurrentReading().ok()?;
            Some([
                r.AccelerationX().ok()? as f32,
                r.AccelerationY().ok()? as f32,
                r.AccelerationZ().ok()? as f32,
            ])
        }
    }
}

#[cfg(all(target_os = "linux", feature = "accel"))]
mod platform {
    use std::{fs, path::Path, path::PathBuf, time::Duration};

    // industrial io sensors exposed through sysfs
    pub struct Sensor {
        axes: [PathBuf; 3],
        // converts raw units to g
        scale: f32,
        name: String,
    }

    pub fn open() -> Option<Sensor> {
        for entry in fs::read_dir("/sys/bus/iio/devices").ok()?.flatten() {
            let dir = entry.path();
            let axes = ["x", "y", "z"].map(|axis| dir.join(format!("in_accel_{axis}_raw")));
            if !axes.iter().all(|p| p.exists()) {
                continue;
            }

            // the kernel scale gives meters per second squared
            let scale = read_f32(&dir.join("in_accel_scale"))
                .or_else(|| read_f32(&dir.join("in_accel_x_scale")))
                .unwrap_or(1.0);
            let name = fs::read_to_string(dir.join("name"))
                .map(|n| n.trim().to_string())
                .unwrap_or_else(|_| "iio accelerometer".to_string());

            let sensor = Sensor {
                axes,
                scale: scale / 9.80665,
                name,
            };
            if sensor.read().is_some() {
                return Some(sensor);
            }
        }
        None
    }

    impl Sensor {
        pub fn name(&self) -> String {
            self.name.clone()
        }

        pub fn interval(&self) -> Duration {
            Duration::from_millis(10)
        }

        pub fn read(&self) -> Option<[f32; 3]> {
            let [x, y, z] = &self.axes;
            Some([
                read_f32(x)? * self.scale,
                read_f32(y)? * self.scale,
                read_f32(z)? * self.scale,
            ])
        }
    }

    fn read_f32(path: &Path) -> Option<f32> {
        fs::read_to_string(path).ok()?.trim().parse().ok()
    }
}

#[cfg(not(any(
    all(windows, feature = "accel"),
    all(target_os = "linux", feature = "accel")
)))]
mod platform {
    use std::time::Duration;

    pub struct Sensor;

    pub fn open() -> Option<Sensor> {
        None
    }

    impl Sensor {
        pub fn name(&self) -> String {
            String::new()
        }

        pub fn interval(&self) -> Duration {
            Duration::from_secs(1)
        }

        pub fn read(&self) -> Option<[f32; 3]> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector(sensitivity: f32) -> (AccelDetector, mpsc::Receiver<Hit>, Arc<Shared>) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::new(sensitivity));
        (
            AccelDetector::new(Duration::from_millis(450), tx, shared.clone()),
            rx,
            shared,
        )
    }

    // feeds readings at 60 hz resting flat with a little jitter
    fn feed(d: &mut AccelDetector, start: Instant, from: u32, to: u32, jolt: impl Fn(u32) -> f32) {
        for i in from..to {
            let jitter = if i % 2 == 0 { 0.005 } else { -0.005 };
            let now = start + Duration::from_millis(i as u64 * 16);
            d.push([jitter, jitter + jolt(i), 1.0], now);
        }
    }

    #[test]
    fn detects_slap() {
        let (mut d, rx, _) = detector(5.0);
        let start = Instant::now();
        feed(&mut d, start, 0, 60, |i| if i == 40 { 0.8 } else { 0.0 });
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn ignores_slow_tilt() {
        let (mut d, rx, _) = detector(10.0);
        let start = Instant::now();
        // tilting the lid over a second
        feed(&mut d, start, 0, 120, |i| {
            i.saturating_sub(30).min(60) as f32 * 0.01
        });
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn ignores_while_playing() {
        let (mut d, rx, shared) = detector(5.0);
        let start = Instant::now();
        shared.playing.store(true, Ordering::Relaxed);
        feed(&mut d, start, 0, 30, |i| if i == 20 { 0.8 } else { 0.0 });
        assert_eq!(rx.try_iter().count(), 0);
    }
}
