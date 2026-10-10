// banner and line by line output for pipes and old terminals

use crate::detector::to_db;
use crate::engine::{Engine, LogKind, LogLine};
use colored::Colorize;
use std::time::{Duration, Instant};

pub const BANNER: &str =
    "███████╗██████╗  █████╗ ███╗   ██╗██╗  ██╗██╗      █████╗ ██████╗ ████████╗ ██████╗ ██████╗
██╔════╝██╔══██╗██╔══██╗████╗  ██║██║ ██╔╝██║     ██╔══██╗██╔══██╗╚══██╔══╝██╔═══██╗██╔══██╗
███████╗██████╔╝███████║██╔██╗ ██║█████╔╝ ██║     ███████║██████╔╝   ██║   ██║   ██║██████╔╝
╚════██║██╔═══╝ ██╔══██║██║╚██╗██║██╔═██╗ ██║     ██╔══██║██╔═══╝    ██║   ██║   ██║██╔═══╝
███████║██║     ██║  ██║██║ ╚████║██║  ██╗███████╗██║  ██║██║        ██║   ╚██████╔╝██║
╚══════╝╚═╝     ╚═╝  ╚═╝╚═╝  ╚═══╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═╝        ╚═╝    ╚═════╝ ╚═╝";

pub fn banner() {
    println!("{}", BANNER);
}

pub fn run_plain(engine: &mut Engine) {
    let mut seq = 0;
    let mut last_level = Instant::now();

    println!("{}", "Press Ctrl+C to stop\n".yellow());

    while !engine.should_quit() {
        engine.tick();

        let lines: Vec<LogLine> = engine.logs_after(seq).cloned().collect();
        for line in lines {
            seq = line.seq;
            let tag = match line.kind {
                LogKind::Hit => "HIT".red().bold(),
                LogKind::Warn => "WARN".yellow().bold(),
                LogKind::Info => "INFO".green().bold(),
            };
            println!("[{}] {} {}", line.time, tag, line.text);
        }

        if last_level.elapsed() >= Duration::from_secs(2) {
            println!(
                "[{}] {} peak {:.0} dB, room {:.0} dB",
                chrono::Local::now().format("%H:%M:%S"),
                "LEVEL".blue().bold(),
                to_db(engine.take_level()),
                to_db(engine.floor())
            );
            last_level = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}
