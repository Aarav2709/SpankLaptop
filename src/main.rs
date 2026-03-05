use std::{
    io::{self, Write},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

mod audio;
mod detector;
mod tui;

use colored::*;
use detector::DetectorState;

fn main() {
    tui::banner();

    println!("{}", "\nSelect a mode:\n".yellow());
    println!("{}", "1. Pain mode".green());
    println!("{}", "2. Sexy mode".green());
    println!("{}", "3. Halo mode".green());
    println!("{}", "4. Custom folder".green());
    println!("{}", "5. Quit".red());

    print!("{}", "\nSelect option: ".purple());
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    let mode = match input.trim() {
        "1" => "pain".to_string(),
        "2" => "sexy".to_string(),
        "3" => "halo".to_string(),
        "4" => {
            println!("Enter folder path:");
            input.clear();
            io::stdin().read_line(&mut input).unwrap();
            input.trim().to_string()
        }
        "5" => return,
        _ => {
            println!("{}", "Invalid option".red());
            return;
        }
    };

    println!(
        "{} {}",
        "Running".green(),
        format!("{} mode", mode).cyan()
    );

    println!("{}", "Press Ctrl+C to stop\n".yellow());

    let sounds = audio::load(&format!("audio/{}", mode))
        .expect("Failed to load audio files");

    let threshold = Arc::new(Mutex::new(0.6)); // Adjust this, works as senstivity.

    let state = Arc::new(Mutex::new(DetectorState {
        impact_level: 0.0,
    }));

    let (tx, rx) = mpsc::channel();

    audio::start_player(rx, sounds);
    detector::start(tx, threshold.clone(), state.clone());

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
