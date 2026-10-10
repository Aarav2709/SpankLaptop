use crate::{
    config::Config,
    detector::to_db,
    engine::{CalibrationView, Command, Engine, LogKind, Status, View},
    plain::BANNER,
};
use ratatui::{
    Frame,
    buffer::Buffer,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};
use std::{collections::VecDeque, time::Duration};

// bottom of every meter in db
const FLOOR_DB: f32 = -70.0;
// how long the border flashes after a hit
const FLASH: Duration = Duration::from_millis(700);
const HISTORY: usize = 600;

const ACCENT: Color = Color::Rgb(255, 122, 69);
const GOOD: Color = Color::Rgb(80, 220, 140);
const WARN: Color = Color::Rgb(255, 196, 0);
const BAD: Color = Color::Rgb(255, 85, 85);
const COOL: Color = Color::Rgb(86, 182, 255);
const DEEP: Color = Color::Rgb(46, 120, 110);
const MAGENTA: Color = Color::Rgb(200, 120, 255);
const MUTED: Color = Color::Rgb(125, 133, 145);
const EDGE: Color = Color::Rgb(70, 76, 88);
const INK: Color = Color::Rgb(18, 20, 24);

// state only the dashboard cares about
#[derive(Default)]
pub struct Ui {
    // recent mic peaks with the newest last
    history: VecDeque<f32>,
    help: bool,
}

impl Ui {
    fn push(&mut self, level: f32) {
        self.history.push_back(level);
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    fn level(&self) -> f32 {
        self.history.back().copied().unwrap_or(0.0)
    }
}

// start screen then the dashboard, all in one terminal session
pub fn run_app(mut config: Config, ask_mode: bool, calibrate: bool) -> anyhow::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = app(&mut terminal, &mut config, ask_mode, calibrate);
    ratatui::restore();
    result
}

fn app(
    terminal: &mut ratatui::DefaultTerminal,
    config: &mut Config,
    ask_mode: bool,
    calibrate: bool,
) -> anyhow::Result<()> {
    if ask_mode {
        if !pick_mode(terminal, config)? {
            return Ok(());
        }
        config.save()?;
    }

    let loading = format!("loading the {} pack", config.mode);
    terminal.draw(|frame| draw_splash(frame, &Splash::Loading(&loading)))?;

    let mut engine = Engine::new(config.clone())?;
    if calibrate {
        engine.command(Command::Calibrate);
    }
    run_loop(terminal, &mut engine)
}

const PACKS: [(&str, &str); 4] = [
    ("Pain", "ow, ouch, hey that hurts"),
    ("Halo", "sounds from the halo pack"),
    ("Custom folder", "your own mp3, wav, ogg or flac files"),
    ("Quit", ""),
];

enum Splash<'a> {
    Menu {
        selected: usize,
        custom: Option<&'a str>,
    },
    Folder {
        input: &'a str,
        error: Option<&'a str>,
    },
    Loading(&'a str),
}

// returns false when the user picks quit
fn pick_mode(terminal: &mut ratatui::DefaultTerminal, config: &mut Config) -> anyhow::Result<bool> {
    let mut selected = match config.mode.as_str() {
        "halo" => 1,
        "custom" => 2,
        _ => 0,
    };
    let mut folder: Option<String> = None;
    let mut error: Option<String> = None;

    loop {
        let screen = match &folder {
            Some(input) => Splash::Folder {
                input,
                error: error.as_deref(),
            },
            None => Splash::Menu {
                selected,
                custom: config.custom_folder.as_deref(),
            },
        };
        terminal.draw(|frame| draw_splash(frame, &screen))?;

        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            Event::Paste(text) => {
                if let Some(input) = &mut folder {
                    input.push_str(&text);
                }
                continue;
            }
            _ => continue,
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }

        // typing a custom folder path
        if let Some(input) = &mut folder {
            match key.code {
                KeyCode::Esc => {
                    folder = None;
                    error = None;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    // windows copy as path wraps the path in quotes
                    let path = input.trim().trim_matches('"').to_string();
                    if path.is_empty() {
                        error = Some("type or paste a folder path".into());
                    } else if !std::path::Path::new(&path).is_dir() {
                        error = Some("that folder does not exist".into());
                    } else {
                        config.mode = "custom".into();
                        config.custom_folder = Some(path);
                        return Ok(true);
                    }
                }
                _ => {}
            }
            continue;
        }

        let choice = match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = (selected + PACKS.len() - 1) % PACKS.len();
                None
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                selected = (selected + 1) % PACKS.len();
                None
            }
            KeyCode::Enter | KeyCode::Char(' ') => Some(selected),
            KeyCode::Char(c @ '1'..='4') => Some(c as usize - '1' as usize),
            KeyCode::Char('q') | KeyCode::Esc => Some(3),
            _ => None,
        };

        match choice {
            Some(0) => config.mode = "pain".into(),
            Some(1) => config.mode = "halo".into(),
            Some(2) => {
                selected = 2;
                folder = Some(config.custom_folder.clone().unwrap_or_default());
                error = None;
                continue;
            }
            Some(_) => return Ok(false),
            None => continue,
        }
        return Ok(true);
    }
}

fn draw_splash(frame: &mut Frame, screen: &Splash) {
    let area = frame.area();
    let banner: Vec<&str> = BANNER.lines().collect();
    let art_width = banner.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let wide = area.width as usize >= art_width + 2;
    let logo_height = if wide { banner.len() as u16 } else { 1 };
    let box_height = match screen {
        Splash::Menu { .. } => PACKS.len() as u16 + 2,
        Splash::Folder { .. } => 6,
        Splash::Loading(_) => 1,
    };

    let [_, logo, _, tagline, _, body, _, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(logo_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(box_height),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // orange fading to amber down the logo
    let logo_lines: Vec<Line> = if wide {
        banner
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let t = i as f32 / (banner.len() - 1).max(1) as f32;
                let g = (122.0 + (196.0 - 122.0) * t) as u8;
                let b = (69.0 * (1.0 - t)) as u8;
                // pad to one width so centering keeps the letters stacked
                Line::from(format!("{:<width$}", line, width = art_width)).fg(Color::Rgb(255, g, b))
            })
            .collect()
    } else {
        vec![Line::from("S P A N K L A P T O P").fg(ACCENT).bold()]
    };
    frame.render_widget(Paragraph::new(logo_lines).centered(), logo);
    frame.render_widget(
        Paragraph::new(Line::from("Hit your laptop. It yells back.").fg(MUTED)).centered(),
        tagline,
    );

    let keys: &[(&str, &str)] = match screen {
        Splash::Menu { selected, custom } => {
            draw_menu(frame, body, *selected, *custom);
            &[
                ("↑ ↓", "move"),
                ("enter", "select"),
                ("1 2 3 4", "pick"),
                ("q", "quit"),
            ]
        }
        Splash::Folder { input, error } => {
            draw_folder(frame, body, input, *error);
            &[("enter", "use this folder"), ("esc", "back")]
        }
        Splash::Loading(text) => {
            let line = Line::from(text.to_string()).fg(MUTED);
            frame.render_widget(Paragraph::new(line).centered(), body);
            &[]
        }
    };
    frame.render_widget(Paragraph::new(key_hints(keys)).centered(), footer);
}

fn draw_menu(frame: &mut Frame, area: Rect, selected: usize, custom: Option<&str>) {
    let rect = centered(area, 60, area.height);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(EDGE))
        .title(Line::from(" Pick a sound pack ").fg(MUTED).bold());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines: Vec<Line> = PACKS
        .iter()
        .enumerate()
        .map(|(i, (name, about))| {
            let active = i == selected;
            // show the saved folder instead of the generic blurb
            let about = match (i, custom) {
                (2, Some(path)) => path,
                _ => about,
            };
            let name_style = if active {
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new().bold()
            };
            Line::from(vec![
                Span::styled(
                    if active { " ▶ " } else { "   " },
                    Style::new().fg(ACCENT).bold(),
                ),
                Span::styled(format!("{}  ", i + 1), Style::new().fg(MUTED)),
                Span::styled(format!("{:<15}", name), name_style),
                Span::styled(about.to_string(), Style::new().fg(MUTED)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_folder(frame: &mut Frame, area: Rect, input: &str, error: Option<&str>) {
    let rect = centered(area, 70, area.height);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(" Custom folder ").fg(ACCENT).bold());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // keep the end of long paths visible while typing
    let room = (inner.width as usize).saturating_sub(6);
    let shown: String = {
        let chars: Vec<char> = input.chars().collect();
        chars[chars.len().saturating_sub(room)..].iter().collect()
    };

    let status = match error {
        Some(err) => Line::from(format!("   {}", err)).fg(BAD),
        None => Line::from("   paste or type the folder that has your sounds").fg(MUTED),
    };
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(" > ", Style::new().fg(ACCENT).bold()),
            Span::styled(shown, Style::new().bold()),
            Span::styled("█", Style::new().fg(ACCENT)),
        ]),
        Line::raw(""),
        status,
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn key_hints(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, label)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    "));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(ACCENT).bold(),
        ));
        spans.push(Span::styled(format!(" {}", label), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn run_loop(terminal: &mut ratatui::DefaultTerminal, engine: &mut Engine) -> anyhow::Result<()> {
    let mut ui = Ui::default();

    while !engine.should_quit() {
        engine.tick();
        ui.push(engine.take_level());

        let view = engine.view(terminal.size()?.height as usize);
        terminal.draw(|frame| draw(frame, &view, &ui))?;

        if !event::poll(Duration::from_millis(25))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let Some(command) = handle_key(&mut ui, key.code, key.modifiers) {
            engine.command(command);
        }
    }

    Ok(())
}

fn handle_key(ui: &mut Ui, code: KeyCode, modifiers: KeyModifiers) -> Option<Command> {
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        return Some(Command::Quit);
    }
    // any key closes the help screen
    if ui.help {
        ui.help = false;
        return (code == KeyCode::Char('q')).then_some(Command::Quit);
    }

    match code {
        KeyCode::Char('q') => Some(Command::Quit),
        KeyCode::Char('?') | KeyCode::Char('h') => {
            ui.help = true;
            None
        }
        KeyCode::Char(' ') | KeyCode::Char('p') => Some(Command::TogglePause),
        KeyCode::Char('n') => Some(Command::NextPack),
        KeyCode::Char('c') => Some(Command::Calibrate),
        KeyCode::Right | KeyCode::Up | KeyCode::Char('+') | KeyCode::Char('=') => {
            Some(Command::SensitivityUp)
        }
        KeyCode::Left | KeyCode::Down | KeyCode::Char('-') | KeyCode::Char('_') => {
            Some(Command::SensitivityDown)
        }
        _ => None,
    }
}

pub fn draw(frame: &mut Frame, view: &View, ui: &Ui) {
    let area = frame.area();
    let compact = area.height < 26;

    let [live, cards, log, settings, keys] = Layout::vertical([
        Constraint::Length(if compact { 8 } else { 12 }),
        Constraint::Length(if compact { 4 } else { 5 }),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_live(frame, live, view, ui);
    draw_cards(frame, cards, view, compact);
    draw_log(frame, log, view);

    draw_settings(frame, settings, view);
    draw_keys(frame, keys);

    if let Some(calibration) = &view.calibration {
        draw_calibration(frame, area, calibration, ui);
    } else if ui.help {
        draw_help(frame, area);
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(EDGE))
        .title(Line::from(format!(" {} ", title)).fg(MUTED).bold())
}

// share of the meter a level fills
fn ratio(level: f32) -> f32 {
    ((to_db(level) - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

fn sep() -> Span<'static> {
    Span::styled("  │  ", Style::new().fg(EDGE))
}

fn draw_live(frame: &mut Frame, area: Rect, view: &View, ui: &Ui) {
    let flash = view.last_hit.filter(|hit| hit.age < FLASH);

    let mut block = panel("Live");
    if let Some(hit) = flash {
        let badge = format!(" HIT  {}  power {} ", hit.source.label(), hit.power);
        block = block.border_style(Style::new().fg(ACCENT)).title(
            Line::from(Span::styled(badge, Style::new().fg(INK).bg(ACCENT).bold())).right_aligned(),
        );
    } else if view.combo > 1 {
        let badge = format!(" COMBO x{} ", view.combo);
        block = block.title(
            Line::from(Span::styled(badge, Style::new().fg(INK).bg(WARN).bold())).right_aligned(),
        );
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width < 8 {
        return;
    }

    let scope = inner;
    draw_scope(frame.buffer_mut(), scope, &ui.history, view.threshold);

    let notice = match view.status {
        Status::NoInput => Some(("no microphone, trying to reconnect".to_string(), BAD)),
        Status::Paused => Some(("paused, press space to listen again".to_string(), WARN)),
        _ if view.mic.is_none() => {
            Some(("listening with the accelerometer only".to_string(), MUTED))
        }
        _ => None,
    };
    if let Some((text, color)) = notice {
        let y = scope.y + scope.height / 2;
        let line = Line::from(Span::styled(
            format!(" {} ", text),
            Style::new().fg(INK).bg(color).bold(),
        ));
        frame.render_widget(
            Paragraph::new(line).centered(),
            Rect::new(scope.x, y, scope.width, 1),
        );
    }
}

// scrolling bar graph of recent peaks, bars that would trigger turn orange
fn draw_scope(buf: &mut Buffer, area: Rect, history: &VecDeque<f32>, threshold: f32) {
    const BARS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let (w, h) = (area.width as usize, area.height as usize);
    if w == 0 || h == 0 {
        return;
    }

    let eighths = |level: f32| (ratio(level) * (h * 8) as f32).round() as usize;

    for (i, &level) in history.iter().rev().take(w).enumerate() {
        let x = area.right() - 1 - i as u16;
        let total = eighths(level);
        let over = level >= threshold;

        for row in 0..h {
            let fill = total.saturating_sub(row * 8).min(8);
            if fill == 0 {
                break;
            }
            let height = row as f32 / h as f32;
            let color = if over {
                ACCENT
            } else if height < 0.35 {
                DEEP
            } else if height < 0.7 {
                GOOD
            } else {
                WARN
            };
            let y = area.bottom() - 1 - row as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(BARS[fill]).set_fg(color);
            }
        }
    }
}

fn draw_cards(frame: &mut Frame, area: Rect, view: &View, compact: bool) {
    let stats = &view.stats;
    let cards = [
        ("Today", stats.today().to_string(), "", COOL),
        ("All time", stats.total.to_string(), "", GOOD),
        ("Hardest", stats.hardest.to_string(), "/100", ACCENT),
        ("Best combo", format!("x{}", stats.best_combo), "", WARN),
    ];
    let cols: [Rect; 4] = Layout::horizontal([Constraint::Ratio(1, 4); 4]).areas(area);

    for ((title, value, suffix, color), col) in cards.into_iter().zip(cols) {
        let block = panel(title);
        let inner = block.inner(col);
        frame.render_widget(block, col);

        let pad = inner;
        let big_width = value.chars().count() * 4;

        let lines =
            if !compact && inner.height >= 3 && big_width + suffix.len() < pad.width as usize {
                big_text(&value)
                    .into_iter()
                    .enumerate()
                    .map(|(row, text)| {
                        let mut spans = vec![Span::styled(text, Style::new().fg(color))];
                        // pad the other rows so centering keeps the digits lined up
                        if row == 2 {
                            spans.push(Span::styled(suffix, Style::new().fg(MUTED)));
                        } else {
                            spans.push(Span::raw(" ".repeat(suffix.len())));
                        }
                        Line::from(spans)
                    })
                    .collect()
            } else {
                vec![Line::from(vec![
                    Span::styled(value, Style::new().fg(color).bold()),
                    Span::styled(suffix, Style::new().fg(MUTED)),
                ])]
            };
        frame.render_widget(Paragraph::new(lines).centered(), pad);
    }
}

fn draw_log(frame: &mut Frame, area: Rect, view: &View) {
    let block = panel("Activity");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let skip = view.log.len().saturating_sub(inner.height as usize);
    let lines: Vec<Line> = view.log[skip..]
        .iter()
        .map(|line| {
            let (badge, color, text) = match line.kind {
                LogKind::Hit => ("HIT ", ACCENT, Style::new().bold()),
                LogKind::Warn => ("WARN", WARN, Style::new().fg(WARN)),
                LogKind::Info => ("INFO", EDGE, Style::new()),
            };
            Line::from(vec![
                Span::styled(format!(" {} ", line.time), Style::new().fg(MUTED)),
                Span::styled(
                    format!(" {} ", badge),
                    Style::new().fg(INK).bg(color).bold(),
                ),
                Span::styled(format!(" {}", line.text), text),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_settings(frame: &mut Frame, area: Rect, view: &View) {
    // one notch per sensitivity step
    let knob = (view.sensitivity.round() as usize).clamp(1, 10) - 1;
    let mut slider = Vec::new();
    for i in 0..10 {
        slider.push(match i.cmp(&knob) {
            std::cmp::Ordering::Less => Span::styled("━", Style::new().fg(ACCENT)),
            std::cmp::Ordering::Equal => Span::styled("●", Style::new().fg(ACCENT).bold()),
            std::cmp::Ordering::Greater => Span::styled("─", Style::new().fg(EDGE)),
        });
    }

    let mut sensors = Vec::new();
    if let Some(mic) = &view.mic {
        sensors.push(mic.split(" (").next().unwrap_or(mic).to_string());
    }
    if view.accel.is_some() {
        sensors.push("accelerometer".to_string());
    }
    let sensors = if sensors.is_empty() {
        "none".to_string()
    } else {
        sensors.join(" + ")
    };

    let mut spans = vec![
        Span::styled("pack ", Style::new().fg(MUTED)),
        Span::styled(view.pack.clone(), Style::new().bold()),
        Span::styled(format!(" ({})", view.pack_len), Style::new().fg(MUTED)),
        sep(),
        Span::styled("sensitivity ", Style::new().fg(MUTED)),
    ];
    spans.extend(slider);
    spans.extend([
        Span::styled(format!(" {:.1}", view.sensitivity), Style::new().bold()),
        sep(),
        Span::styled("cooldown ", Style::new().fg(MUTED)),
        Span::raw(format!("{} ms", view.cooldown_ms)),
        sep(),
        Span::styled(sensors, Style::new().fg(MUTED)),
    ]);

    frame.render_widget(Paragraph::new(Line::from(spans)).centered(), area);
}

fn draw_keys(frame: &mut Frame, area: Rect) {
    let keys = [
        ("space", "pause"),
        ("← →", "sensitivity"),
        ("n", "pack"),
        ("c", "calibrate"),
        ("?", "help"),
        ("q", "quit"),
    ];
    frame.render_widget(Paragraph::new(key_hints(&keys)).centered(), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (w, h) = (width.min(area.width), height.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

// a horizontal bar of the given width filled to a share
fn gauge(width: usize, share: f32, color: Color) -> Vec<Span<'static>> {
    let filled = ((share.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(color)),
        Span::styled("─".repeat(width - filled), Style::new().fg(EDGE)),
    ]
}

fn draw_calibration(frame: &mut Frame, area: Rect, calibration: &CalibrationView, ui: &Ui) {
    let rect = centered(area, 62, 13);
    frame.render_widget(Clear, rect);

    let done = matches!(calibration, CalibrationView::Done { .. });
    let color = if done { GOOD } else { MAGENTA };
    let footer = if done {
        " c calibrates again "
    } else {
        " c cancels "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Line::from(" Calibration ").fg(color).bold())
        .title_bottom(Line::from(footer).fg(MUTED).right_aligned());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let step = |done: bool, active: bool, n: &str, text: &str| {
        let (mark, color) = if done {
            ("✓", GOOD)
        } else if active {
            ("▶", MAGENTA)
        } else {
            ("·", EDGE)
        };
        Line::from(vec![
            Span::styled(format!("  {} ", mark), Style::new().fg(color).bold()),
            Span::styled(format!("{}  ", n), Style::new().fg(MUTED)),
            Span::styled(
                text.to_string(),
                if active {
                    Style::new().bold()
                } else {
                    Style::new().fg(MUTED)
                },
            ),
        ])
    };
    let indent = || Span::raw("       ");
    let db_list = |levels: &[f32]| {
        levels
            .iter()
            .map(|l| format!("{:.0} dB", to_db(*l)))
            .collect::<Vec<_>>()
            .join("   ")
    };

    let bar_width = (inner.width as usize).saturating_sub(12).min(44);
    let mut lines = vec![Line::raw("")];

    match calibration {
        CalibrationView::Quiet { progress } => {
            lines.push(step(false, true, "1", "Stay quiet so it can hear the room"));
            let mut bar = vec![indent()];
            bar.extend(gauge(bar_width, *progress, MAGENTA));
            lines.push(Line::from(bar));
            lines.push(Line::raw(""));
            lines.push(step(false, false, "2", "Hit the laptop 3 times"));
        }
        CalibrationView::Listen { progress, best } => {
            lines.push(step(true, false, "1", "Room measured"));
            lines.push(Line::raw(""));
            lines.push(step(
                false,
                true,
                "2",
                "Hit the laptop 3 times, like you normally would",
            ));

            // live level so you can see each hit land
            let level = ui.level();
            let mut meter = vec![indent(), Span::styled("level ", Style::new().fg(MUTED))];
            meter.extend(gauge(bar_width.saturating_sub(6), ratio(level), COOL));
            lines.push(Line::from(meter));

            let mut time = vec![indent(), Span::styled("time  ", Style::new().fg(MUTED))];
            time.extend(gauge(bar_width.saturating_sub(6), 1.0 - progress, MAGENTA));
            lines.push(Line::from(time));
            lines.push(Line::raw(""));

            let heard = if best.is_empty() {
                Span::styled("waiting for a hit", Style::new().fg(MUTED))
            } else {
                Span::styled(db_list(best), Style::new().fg(GOOD).bold())
            };
            lines.push(Line::from(vec![
                indent(),
                Span::styled("loudest hits  ", Style::new().fg(MUTED)),
                heard,
            ]));
        }
        CalibrationView::Done {
            taps,
            trigger,
            sensitivity,
        } => {
            lines.push(step(true, false, "1", "Room measured"));
            lines.push(step(true, false, "2", "Hits measured"));
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                indent(),
                Span::styled("your hits      ", Style::new().fg(MUTED)),
                Span::styled(db_list(taps), Style::new().bold()),
            ]));
            lines.push(Line::from(vec![
                indent(),
                Span::styled("triggers at    ", Style::new().fg(MUTED)),
                Span::styled(
                    format!("{:.0} dB", to_db(*trigger)),
                    Style::new().fg(ACCENT).bold(),
                ),
            ]));
            lines.push(Line::from(vec![
                indent(),
                Span::styled("sensitivity    ", Style::new().fg(MUTED)),
                Span::styled(format!("{:.1}", sensitivity), Style::new().fg(GOOD).bold()),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rect = centered(area, 70, 21);
    frame.render_widget(Clear, rect);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(" Help ").fg(ACCENT).bold())
        .title_bottom(Line::from(" any key closes ").fg(MUTED).right_aligned());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let heading = |text: &str| {
        Line::from(Span::styled(
            format!(" {}", text),
            Style::new().fg(ACCENT).bold(),
        ))
    };
    let key = |k: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!("   {:<12}", k), Style::new().bold()),
            Span::styled(text.to_string(), Style::new().fg(MUTED)),
        ])
    };
    let tip = |text: &str| Line::from(Span::styled(format!("   {}", text), Style::new().fg(MUTED)));

    let lines = vec![
        heading("Keys"),
        key("space  p", "pause and resume"),
        key("← →  + -", "sensitivity down and up"),
        key("n", "next sound pack"),
        key("c", "calibrate to how hard you hit"),
        key("?  h", "this screen"),
        key("q", "quit"),
        Line::raw(""),
        heading("It never triggers"),
        tip("Press c to calibrate, or press → until your hits turn the bars"),
        tip("orange."),
        tip("On Windows, turning off audio enhancements for the mic"),
        tip("(Sound settings, your mic, Audio enhancements) helps a lot."),
        Line::raw(""),
        heading("It triggers too easily"),
        tip("Press ← until typing and talking stop turning the bars orange."),
        Line::raw(""),
        heading("Settings"),
        tip("Sensitivity, pack and calibration are saved automatically."),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

// tall digits drawn with half blocks, three rows high
fn big_text(text: &str) -> [String; 3] {
    const GLYPHS: [(char, [u8; 5]); 11] = [
        ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
        ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
        ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
        ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
        ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
        ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
        ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
        ('7', [0b111, 0b001, 0b001, 0b001, 0b001]),
        ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
        ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
        ('x', [0b000, 0b101, 0b010, 0b101, 0b000]),
    ];

    let mut rows = [String::new(), String::new(), String::new()];
    for c in text.chars() {
        let glyph = GLYPHS
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, g)| *g)
            .unwrap_or([0; 5]);

        for (r, row) in rows.iter_mut().enumerate() {
            let top = glyph[r * 2];
            let bottom = glyph.get(r * 2 + 1).copied().unwrap_or(0);
            for bit in (0..3).rev() {
                row.push(match (top >> bit & 1, bottom >> bit & 1) {
                    (1, 1) => '█',
                    (1, 0) => '▀',
                    (0, 1) => '▄',
                    _ => ' ',
                });
            }
            row.push(' ');
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        detector::HitSource,
        engine::{LastHit, LogLine},
        stats::Stats,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn view() -> View {
        View {
            pack: "pain".into(),
            pack_len: 10,
            status: Status::Listening,
            sensitivity: 5.0,
            cooldown_ms: 450,
            mic: Some("Test mic (F32, 2ch @ 48000 Hz)".into()),
            accel: None,
            threshold: 0.03,
            combo: 3,
            last_hit: Some(LastHit {
                age: Duration::from_millis(100),
                power: 88,
                source: HitSource::Mic,
            }),
            calibration: None,
            stats: Stats {
                total: 42,
                hardest: 88,
                best_combo: 7,
                ..Stats::default()
            },
            log: vec![LogLine {
                seq: 1,
                time: "12:00:00".into(),
                kind: LogKind::Hit,
                text: "mic hit, power 88".into(),
            }],
        }
    }

    fn ui() -> Ui {
        let mut ui = Ui::default();
        for i in 0..200 {
            ui.push(if i % 40 == 0 { 0.5 } else { 0.002 });
        }
        ui
    }

    fn render(width: u16, height: u16, view: &View, ui: &Ui) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, view, ui)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn dashboard_shows_state() {
        let screen = render(110, 32, &view(), &ui());
        for gone in [
            "SPANKLAPTOP",
            "LISTENING",
            "trigger",
            "loudest",
            "Last 7 days",
        ] {
            assert!(!screen.contains(gone), "still shows {}\n{}", gone, screen);
        }
        for text in [
            "HIT  mic  power 88",
            "Activity",
            "mic hit, power 88",
            "Test mic",
            "space pause",
        ] {
            assert!(screen.contains(text), "missing {}\n{}", text, screen);
        }
    }

    #[test]
    fn overlays_render() {
        let mut v = view();
        v.status = Status::Calibrating;
        v.calibration = Some(CalibrationView::Listen {
            progress: 0.5,
            best: vec![0.1, 0.05],
        });
        let screen = render(100, 30, &v, &ui());
        assert!(
            screen.contains("Calibration") && screen.contains("-20 dB"),
            "{}",
            screen
        );

        v.calibration = Some(CalibrationView::Done {
            taps: vec![0.1, 0.05, 0.04],
            trigger: 0.02,
            sensitivity: 6.6,
        });
        let screen = render(100, 30, &v, &ui());
        assert!(
            screen.contains("6.6") && screen.contains("-34 dB"),
            "{}",
            screen
        );

        let mut help = ui();
        help.help = true;
        let screen = render(100, 30, &view(), &help);
        assert!(screen.contains("It never triggers"), "{}", screen);
    }

    #[test]
    fn small_terminals_do_not_panic() {
        for (w, h) in [(1, 1), (10, 3), (40, 12), (80, 24)] {
            render(w, h, &view(), &ui());
        }
    }

    #[test]
    fn big_digits_are_three_rows() {
        let rows = big_text("x10");
        assert!(rows.iter().all(|r| r.chars().count() == 12));
        assert!(rows[0].contains('█') || rows[0].contains('▀'));
    }

    fn render_splash(width: u16, height: u16, screen: &Splash) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw_splash(f, screen)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn start_screen_renders() {
        let menu = Splash::Menu {
            selected: 1,
            custom: Some("C:\\sounds"),
        };
        let screen = render_splash(110, 30, &menu);
        for text in [
            "███████╗",
            "Pick a sound pack",
            "▶ 2  Halo",
            "C:\\sounds",
            "enter select",
        ] {
            assert!(screen.contains(text), "missing {}\n{}", text, screen);
        }

        // narrow terminals get the short title instead of the art
        let screen = render_splash(60, 20, &menu);
        assert!(screen.contains("S P A N K L A P T O P"), "{}", screen);

        let folder = Splash::Folder {
            input: "D:\\clips",
            error: Some("that folder does not exist"),
        };
        let screen = render_splash(100, 30, &folder);
        assert!(
            screen.contains("> D:\\clips") && screen.contains("does not exist"),
            "{}",
            screen
        );

        for (w, h) in [(1, 1), (20, 5)] {
            render_splash(w, h, &menu);
            render_splash(w, h, &Splash::Loading("loading"));
        }
    }
}
