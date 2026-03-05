use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use colored::Colorize;
use std::{
    sync::{mpsc::Sender, Arc, Mutex},
    time::{Duration, Instant},
};

pub struct DetectorState {
    pub impact_level: f32,
}

pub fn start(
    tx: Sender<()>,
    threshold: Arc<Mutex<f32>>,
    state: Arc<Mutex<DetectorState>>,
) {
    std::thread::spawn(move || {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .expect("No input device available");

        let config = device.default_input_config().unwrap();

        let mut last_hit = Instant::now();
        let mut smoothed = 0.0;

        let err_fn = |err| eprintln!("Stream error: {}", err);

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    let amp = data
                        .iter()
                        .map(|s| s.abs())
                        .fold(0.0, f32::max);

                    // smoothing filter
                    smoothed = smoothed * 0.8 + amp * 0.2;

                    state.lock().unwrap().impact_level = smoothed;

                    let threshold = *threshold.lock().unwrap();

                    if smoothed > threshold
                        && last_hit.elapsed() > Duration::from_millis(450)
                    {
                        let _ = tx.send(());

                        let now = chrono::Local::now();

                        println!(
                            "[{}] {} {:.2}",
                            now.format("%H:%M:%S"),
                            "IMPACT".red().bold(),
                            smoothed
                        );

                        last_hit = Instant::now();
                    }
                },
                err_fn,
                None,
            )
            .unwrap();

        stream.play().unwrap();

        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}
