use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
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
        if let Err(err) = run_detector(tx, threshold, state) {
            eprintln!("Detector failed: {err}");
        }
    });
}

fn run_detector(
    tx: Sender<()>,
    threshold: Arc<Mutex<f32>>,
    state: Arc<Mutex<DetectorState>>,
) -> anyhow::Result<()> {
    let host = cpal::default_host();

    let device = host
        .default_input_device()
        .context("No input device available")?;

    let config = device.default_input_config()?;
    let stream_config = config.config();

    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
    println!(
        "Mic ready: {} ({:?}, {}ch @ {} Hz)",
        device_name,
        config.sample_format(),
        stream_config.channels,
        stream_config.sample_rate.0
    );

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let tx_cb = tx.clone();
            let threshold_cb = threshold.clone();
            let state_cb = state.clone();
            let mut last_hit = Instant::now();
            let mut smoothed = 0.0;
            let mut last_level_log = Instant::now();

            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    let amp = data
                        .iter()
                        .map(|sample| sample.abs())
                        .fold(0.0, f32::max);

                    process_amplitude(
                        amp,
                        &tx_cb,
                        &threshold_cb,
                        &state_cb,
                        &mut smoothed,
                        &mut last_hit,
                        &mut last_level_log,
                    );
                },
                |err| eprintln!("Stream error: {}", err),
                None,
            )?
        }
        SampleFormat::I16 => {
            let tx_cb = tx.clone();
            let threshold_cb = threshold.clone();
            let state_cb = state.clone();
            let mut last_hit = Instant::now();
            let mut smoothed = 0.0;
            let mut last_level_log = Instant::now();

            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    let amp = data
                        .iter()
                        .map(|sample| (*sample as f32 / i16::MAX as f32).abs())
                        .fold(0.0, f32::max);

                    process_amplitude(
                        amp,
                        &tx_cb,
                        &threshold_cb,
                        &state_cb,
                        &mut smoothed,
                        &mut last_hit,
                        &mut last_level_log,
                    );
                },
                |err| eprintln!("Stream error: {}", err),
                None,
            )?
        }
        SampleFormat::U16 => {
            let tx_cb = tx.clone();
            let threshold_cb = threshold.clone();
            let state_cb = state.clone();
            let mut last_hit = Instant::now();
            let mut smoothed = 0.0;
            let mut last_level_log = Instant::now();

            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    let amp = data
                        .iter()
                        .map(|sample| {
                            let centered = (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0;
                            centered.abs()
                        })
                        .fold(0.0, f32::max);

                    process_amplitude(
                        amp,
                        &tx_cb,
                        &threshold_cb,
                        &state_cb,
                        &mut smoothed,
                        &mut last_hit,
                        &mut last_level_log,
                    );
                },
                |err| eprintln!("Stream error: {}", err),
                None,
            )?
        }
        other => anyhow::bail!("Unsupported input sample format: {:?}", other),
    };

    stream.play()?;

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn process_amplitude(
    amp: f32,
    tx: &Sender<()>,
    threshold: &Arc<Mutex<f32>>,
    state: &Arc<Mutex<DetectorState>>,
    smoothed: &mut f32,
    last_hit: &mut Instant,
    last_level_log: &mut Instant,
) {
    // Exponential smoothing keeps impact detection stable across mic noise.
    *smoothed = *smoothed * 0.8 + amp * 0.2;

    if let Ok(mut current) = state.lock() {
        current.impact_level = *smoothed;
    }

    let threshold_value = threshold.lock().map(|value| *value).unwrap_or(0.6);

    if last_level_log.elapsed() >= Duration::from_secs(2) {
        let now = chrono::Local::now();
        println!(
            "[{}] {} {:.2} (thr {:.2})",
            now.format("%H:%M:%S"),
            "LEVEL".blue().bold(),
            *smoothed,
            threshold_value
        );
        *last_level_log = Instant::now();
    }

    if *smoothed > threshold_value && last_hit.elapsed() > Duration::from_millis(450)
    {
        let _ = tx.send(());

        let now = chrono::Local::now();

        println!(
            "[{}] {} {:.2}",
            now.format("%H:%M:%S"),
            "IMPACT".red().bold(),
            *smoothed
        );

        *last_hit = Instant::now();
    }
}
