// start at login so the tray icon is always there

pub use platform::{install, is_installed, uninstall};

// the command the system runs at login
fn command() -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" --tray", exe.display()))
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE: &str = "SpankLaptop";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn install() -> anyhow::Result<()> {
        let data = wide(&super::command()?);
        let (key, value) = (wide(RUN_KEY), wide(VALUE));
        // safety all pointers come from live nul terminated buffers
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        anyhow::ensure!(
            status == ERROR_SUCCESS,
            "Could not write the startup entry (error {})",
            status
        );
        Ok(())
    }

    pub fn uninstall() -> anyhow::Result<()> {
        if !is_installed() {
            return Ok(());
        }
        let (key, value) = (wide(RUN_KEY), wide(VALUE));
        // safety both pointers come from live nul terminated buffers
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        anyhow::ensure!(
            status == ERROR_SUCCESS,
            "Could not remove the startup entry (error {})",
            status
        );
        Ok(())
    }

    pub fn is_installed() -> bool {
        let (key, value) = (wide(RUN_KEY), wide(VALUE));
        // safety null outputs only ask whether the value exists
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        status == ERROR_SUCCESS
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::Context;
    use std::path::PathBuf;

    // xdg autostart entry
    fn entry() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("autostart").join("spanklaptop.desktop"))
    }

    pub fn install() -> anyhow::Result<()> {
        let path = entry().context("No config folder on this system")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=SpankLaptop\nComment=Hit your laptop. It yells back.\nExec={}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            super::command()?
        );
        std::fs::write(&path, desktop).with_context(|| format!("Cannot write {}", path.display()))
    }

    pub fn uninstall() -> anyhow::Result<()> {
        match entry() {
            Some(path) if path.exists() => Ok(std::fs::remove_file(path)?),
            _ => Ok(()),
        }
    }

    pub fn is_installed() -> bool {
        entry().is_some_and(|p| p.exists())
    }
}
