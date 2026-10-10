use std::io::{self, IsTerminal, Write};

mod accel;
mod audio;
mod config;
mod detector;
mod engine;
mod plain;
mod startup;
mod stats;
mod tray;
mod tui;

use anyhow::Context;
use colored::*;
use config::{Config, SourceMode};
use engine::{Command, Engine};

const USAGE: &str = "Usage: SpankLaptop [options]

Options:
  -m, --mode <pain|halo|folder>   sound pack, skips the menu
  -s, --sensitivity <1-10>        1 needs hard hits, 10 reacts to light taps
  -c, --cooldown <ms>             minimum time between sounds
      --mic <name>                use the mic whose name contains this text
      --source <auto|mic|accel>   which sensors to listen with
      --calibrate                 measure your hits on startup
      --plain                     simple scrolling output instead of the dashboard
      --tray                      run in the background with a tray icon
      --list-mics                 show available microphones
      --stats                     show your hit stats
      --install-startup           start in the tray when you log in
      --uninstall-startup         stop starting at login
  -h, --help                      show this help
  -V, --version                   show the version

Settings are saved in the config file shown by --stats. Flags override them for one run.";

#[derive(Default)]
struct Args {
    mode: Option<String>,
    sensitivity: Option<f32>,
    cooldown_ms: Option<u64>,
    mic: Option<String>,
    source: Option<SourceMode>,
    calibrate: bool,
    plain: bool,
    tray: bool,
    action: Option<Action>,
}

enum Action {
    Help,
    Version,
    ListMics,
    Stats,
    InstallStartup,
    UninstallStartup,
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => fail(&err, false),
    };
    let tray = args.tray;
    if let Err(err) = run(args) {
        fail(&err, tray);
    }
}

fn fail(err: &anyhow::Error, tray: bool) -> ! {
    if tray {
        tray::show_error(&format!("{:#}", err));
    } else {
        eprintln!("{} {:#}", "Error:".red().bold(), err);
    }
    std::process::exit(1);
}

fn run(args: Args) -> anyhow::Result<()> {
    if let Some(action) = args.action {
        return run_action(action);
    }

    let mut config = Config::load()?;
    if let Some(mode) = &args.mode {
        if audio::BUILT_IN.contains(&mode.as_str()) || mode == "custom" {
            config.mode = mode.clone();
        } else {
            config.mode = "custom".to_string();
            config.custom_folder = Some(mode.clone());
        }
    }
    if let Some(s) = args.sensitivity {
        config.sensitivity = s;
    }
    if let Some(ms) = args.cooldown_ms {
        config.cooldown_ms = ms;
    }
    if let Some(mic) = args.mic {
        config.mic = Some(mic);
    }
    if let Some(source) = args.source {
        config.source = source;
    }

    if args.tray {
        if tray::detach_console()? {
            println!("SpankLaptop is running in the tray");
            return Ok(());
        }
        return tray::run(Engine::new(config)?);
    }

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if interactive && !args.plain {
        return tui::run_app(config, args.mode.is_none(), args.calibrate);
    }

    plain::banner();
    if args.mode.is_none() && interactive {
        if !pick_mode(&mut config)? {
            return Ok(());
        }
        config.save()?;
    }

    println!("{}", "Starting...".green());
    let mut engine = Engine::new(config)?;
    if args.calibrate {
        engine.command(Command::Calibrate);
    }
    plain::run_plain(&mut engine);
    Ok(())
}

fn run_action(action: Action) -> anyhow::Result<()> {
    match action {
        Action::Help => println!("{}", USAGE),
        Action::Version => println!("SpankLaptop {}", env!("CARGO_PKG_VERSION")),
        Action::ListMics => {
            for name in detector::input_names()? {
                println!("{}", name);
            }
        }
        Action::Stats => print_stats()?,
        Action::InstallStartup => {
            startup::install()?;
            println!("SpankLaptop will start in the tray when you log in");
        }
        Action::UninstallStartup => {
            startup::uninstall()?;
            println!("SpankLaptop will no longer start at login");
        }
    }
    Ok(())
}

fn print_stats() -> anyhow::Result<()> {
    let stats = stats::Stats::load()?;
    println!("{} {}", "Today:".cyan(), stats.today());
    println!("{} {}", "All time:".cyan(), stats.total);
    if stats.hardest > 0 {
        println!(
            "{} {}/100 on {}",
            "Hardest:".cyan(),
            stats.hardest,
            stats.hardest_at
        );
    }
    println!("{} x{}", "Best combo:".cyan(), stats.best_combo);

    let days = stats.last_days(7);
    let most = days.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    println!("\n{}", "Last 7 days".dimmed());
    for (day, count) in days {
        let bar = "█".repeat((count as usize * 30).div_ceil(most as usize));
        println!("{} {} {}", day.format("%a %d"), bar.cyan(), count);
    }

    let login = if startup::is_installed() { "yes" } else { "no" };
    println!("\n{} {}", "Starts at login:".dimmed(), login);
    if let Some(path) = config::dir() {
        println!("{} {}", "Config folder:".dimmed(), path.display());
    }
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut parsed = Args::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .with_context(|| format!("{} needs a value", name))
        };
        match arg.as_str() {
            "-h" | "--help" => parsed.action = Some(Action::Help),
            "-V" | "--version" => parsed.action = Some(Action::Version),
            "--list-mics" => parsed.action = Some(Action::ListMics),
            "--stats" => parsed.action = Some(Action::Stats),
            "--install-startup" => parsed.action = Some(Action::InstallStartup),
            "--uninstall-startup" => parsed.action = Some(Action::UninstallStartup),
            "--calibrate" => parsed.calibrate = true,
            "--plain" => parsed.plain = true,
            "--tray" => parsed.tray = true,
            "-m" | "--mode" => parsed.mode = Some(value("--mode")?),
            "--mic" => parsed.mic = Some(value("--mic")?),
            "--source" => {
                let text = value("--source")?;
                parsed.source =
                    Some(SourceMode::parse(&text).context("--source must be auto, mic or accel")?);
            }
            "-s" | "--sensitivity" => {
                let s: f32 = value("--sensitivity")?
                    .parse()
                    .context("--sensitivity must be a number")?;
                anyhow::ensure!(
                    (1.0..=10.0).contains(&s),
                    "--sensitivity must be between 1 and 10"
                );
                parsed.sensitivity = Some(s);
            }
            "-c" | "--cooldown" => {
                parsed.cooldown_ms = Some(
                    value("--cooldown")?
                        .parse()
                        .context("--cooldown must be whole milliseconds")?,
                );
            }
            other => anyhow::bail!("Unknown argument '{}'\n\n{}", other, USAGE),
        }
    }

    Ok(parsed)
}

// returns false when the user picks quit
fn pick_mode(config: &mut Config) -> anyhow::Result<bool> {
    let current = match config.mode.as_str() {
        "halo" => "2",
        "custom" => "3",
        _ => "1",
    };

    println!("{}", "\nSelect a mode:\n".yellow());
    println!("{}", "1. Pain mode".green());
    println!("{}", "2. Halo mode".green());
    println!("{}", "3. Custom folder".green());
    println!("{}", "4. Quit".red());

    let choice = prompt(&format!("\nSelect option [{}]: ", current))?;
    let choice = if choice.is_empty() {
        current
    } else {
        choice.as_str()
    };

    match choice {
        "1" => config.mode = "pain".to_string(),
        "2" => config.mode = "halo".to_string(),
        "3" => {
            let saved = config.custom_folder.clone().unwrap_or_default();
            let hint = if saved.is_empty() {
                String::new()
            } else {
                format!(" [{}]", saved)
            };
            let path = prompt(&format!("Enter folder path{}: ", hint))?;
            // windows copy as path wraps the path in quotes
            let path = path.trim_matches('"');
            let path = if path.is_empty() {
                saved
            } else {
                path.to_string()
            };
            anyhow::ensure!(!path.is_empty(), "No folder given");
            config.mode = "custom".to_string();
            config.custom_folder = Some(path);
        }
        "4" => return Ok(false),
        _ => anyhow::bail!("Invalid option"),
    }
    Ok(true)
}

fn prompt(text: &str) -> anyhow::Result<String> {
    print!("{}", text.purple());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}
