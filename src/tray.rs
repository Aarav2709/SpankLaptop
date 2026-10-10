// runs in the background with a tray icon instead of a terminal

use crate::engine::Engine;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(15);

#[cfg(feature = "tray")]
fn status_text(engine: &Engine) -> String {
    let state = if engine.is_paused() {
        "paused"
    } else {
        "listening"
    };
    format!(
        "{} pack, {} hits today, {}",
        engine.pack_name(),
        engine.hits_today(),
        state
    )
}

// orange badge with an exclamation mark drawn at runtime
#[cfg(feature = "tray")]
fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let c = (s - 1.0) / 2.0;
    let mut data = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32, y as f32);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            let mark = (fx - c).abs() <= s * 0.07
                && ((fy >= s * 0.22 && fy <= s * 0.6) || (fy >= s * 0.7 && fy <= s * 0.8));

            let pixel = if d > s / 2.0 {
                [0, 0, 0, 0]
            } else if mark || d > s / 2.0 - 2.0 {
                [255, 255, 255, 255]
            } else {
                [235, 87, 54, 255]
            };
            data.extend_from_slice(&pixel);
        }
    }
    data
}

#[cfg(all(windows, feature = "tray"))]
pub fn run(mut engine: Engine) -> anyhow::Result<()> {
    use crate::{engine::Command, startup};
    use tray_icon::{
        Icon, TrayIconBuilder,
        menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    let status = MenuItem::new(status_text(&engine), false, None);
    let pause = MenuItem::new("Pause", true, None);
    let next = MenuItem::new("Next pack", true, None);
    let login = CheckMenuItem::new("Start at login", true, startup::is_installed(), None);
    let quit = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &pause,
        &next,
        &login,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("SpankLaptop")
        .with_icon(Icon::from_rgba(icon_rgba(32), 32, 32)?)
        .build()?;

    let mut last_status = String::new();

    while !engine.should_quit() {
        // the tray icon needs a message loop on this thread
        // safety msg is a plain struct and every call gets a valid pointer to it
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *pause.id() {
                engine.command(Command::TogglePause);
                pause.set_text(if engine.is_paused() {
                    "Resume"
                } else {
                    "Pause"
                });
            } else if event.id == *next.id() {
                engine.command(Command::NextPack);
            } else if event.id == *login.id() {
                // muda already flipped the checkmark
                let result = if login.is_checked() {
                    startup::install()
                } else {
                    startup::uninstall()
                };
                if result.is_err() {
                    login.set_checked(startup::is_installed());
                }
            } else if event.id == *quit.id() {
                engine.command(Command::Quit);
            }
        }

        engine.tick();

        let text = status_text(&engine);
        if text != last_status {
            status.set_text(&text);
            let _ = tray.set_tooltip(Some(format!("SpankLaptop: {}", text)));
            last_status = text;
        }

        std::thread::sleep(TICK);
    }

    Ok(())
}

#[cfg(all(target_os = "linux", feature = "tray"))]
pub fn run(mut engine: Engine) -> anyhow::Result<()> {
    use crate::{engine::Command, startup};
    use ksni::blocking::TrayMethods;
    use ksni::menu::{CheckmarkItem, StandardItem};
    use std::sync::mpsc;

    enum Action {
        Engine(Command),
        Login(bool),
    }

    struct AppTray {
        tx: mpsc::Sender<Action>,
        status: String,
        paused: bool,
        login: bool,
    }

    impl AppTray {
        fn send(&self, action: Action) {
            let _ = self.tx.send(action);
        }
    }

    impl ksni::Tray for AppTray {
        fn id(&self) -> String {
            "spanklaptop".into()
        }

        fn title(&self) -> String {
            "SpankLaptop".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            // ksni wants argb so move alpha to the front
            let mut data = icon_rgba(32);
            for px in data.as_chunks_mut::<4>().0 {
                px.rotate_right(1);
            }
            vec![ksni::Icon {
                width: 32,
                height: 32,
                data,
            }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "SpankLaptop".into(),
                description: self.status.clone(),
                ..Default::default()
            }
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: self.status.clone(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: if self.paused { "Resume" } else { "Pause" }.into(),
                    activate: Box::new(|t: &mut Self| t.send(Action::Engine(Command::TogglePause))),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Next pack".into(),
                    activate: Box::new(|t: &mut Self| t.send(Action::Engine(Command::NextPack))),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Start at login".into(),
                    checked: self.login,
                    activate: Box::new(|t: &mut Self| {
                        t.login = !t.login;
                        t.send(Action::Login(t.login));
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(|t: &mut Self| t.send(Action::Engine(Command::Quit))),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    let (tx, rx) = mpsc::channel();
    let tray = AppTray {
        tx,
        status: status_text(&engine),
        paused: false,
        login: startup::is_installed(),
    };

    // keep working without an icon on desktops that have no tray
    let handle = match tray.spawn() {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!(
                "No system tray available ({}), running without an icon",
                err
            );
            None
        }
    };

    let mut last_status = String::new();

    while !engine.should_quit() {
        while let Ok(action) = rx.try_recv() {
            match action {
                Action::Engine(command) => engine.command(command),
                Action::Login(on) => {
                    let result = if on {
                        startup::install()
                    } else {
                        startup::uninstall()
                    };
                    if let Err(err) = result {
                        eprintln!("{:#}", err);
                    }
                }
            }
        }

        engine.tick();

        let text = status_text(&engine);
        if text != last_status {
            if let Some(handle) = &handle {
                let (status, paused) = (text.clone(), engine.is_paused());
                handle.update(move |t| {
                    t.status = status;
                    t.paused = paused;
                    t.login = startup::is_installed();
                });
            }
            last_status = text;
        }

        std::thread::sleep(TICK);
    }

    if let Some(handle) = handle {
        handle.shutdown().wait();
    }
    Ok(())
}

// no tray support so just run quietly
#[cfg(not(any(
    all(windows, feature = "tray"),
    all(target_os = "linux", feature = "tray")
)))]
pub fn run(mut engine: Engine) -> anyhow::Result<()> {
    while !engine.should_quit() {
        engine.tick();
        std::thread::sleep(TICK);
    }
    Ok(())
}

// frees the terminal so the tray app can live on its own
#[cfg(windows)]
pub fn detach_console() -> anyhow::Result<bool> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut ids = [0u32; 4];
    // safety the buffer length matches the array
    let attached = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };

    if attached > 1 {
        // started from a shell so relaunch without a console and let the shell go
        std::process::Command::new(std::env::current_exe()?)
            .args(std::env::args_os().skip(1))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        return Ok(true);
    }

    // started at login with its own console window so close it
    // safety freeing a console we own has no preconditions
    unsafe { FreeConsole() };
    Ok(false)
}

#[cfg(not(windows))]
pub fn detach_console() -> anyhow::Result<bool> {
    Ok(false)
}

// errors in tray mode have no terminal to land in
#[cfg(windows)]
pub fn show_error(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (body, title) = (wide(text), wide("SpankLaptop"));
    // safety both strings are nul terminated and outlive the call
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[cfg(not(windows))]
pub fn show_error(text: &str) {
    eprintln!("{}", text);
}

#[cfg(all(test, feature = "tray"))]
mod tests {
    #[test]
    fn icon_has_the_right_size() {
        let data = super::icon_rgba(32);
        assert_eq!(data.len(), 32 * 32 * 4);
        // corners are transparent and the middle is drawn
        assert_eq!(data[3], 0);
        assert_eq!(data[(16 * 32 + 16) * 4 + 3], 255);
    }
}
