use anyhow::Context;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    #[default]
    Auto,
    Mic,
    Accel,
}

impl SourceMode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "mic" => Some(Self::Mic),
            "accel" => Some(Self::Accel),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // pain, halo or custom
    pub mode: String,
    pub custom_folder: Option<String>,
    pub sensitivity: f32,
    pub cooldown_ms: u64,
    // part of the mic name, empty means the system default
    pub mic: Option<String>,
    pub source: SourceMode,
    pub volume: f32,
    pub dynamic_volume: bool,
    pub combo: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: "pain".to_string(),
            custom_folder: None,
            sensitivity: 5.0,
            cooldown_ms: 450,
            mic: None,
            source: SourceMode::Auto,
            volume: 1.0,
            dynamic_volume: true,
            combo: true,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let mut config: Self = read("config.toml")?;
        config.sensitivity = config.sensitivity.clamp(1.0, 10.0);
        config.volume = config.volume.clamp(0.0, 2.0);
        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        write("config.toml", self)
    }
}

// where config and stats live
pub fn dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("SpankLaptop"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("spanklaptop"))
    }
}

pub fn path(name: &str) -> Option<PathBuf> {
    dir().map(|d| d.join(name))
}

// missing files give defaults so first runs just work
pub fn read<T: DeserializeOwned + Default>(name: &str) -> anyhow::Result<T> {
    let Some(path) = path(name) else {
        return Ok(T::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("{} is invalid, fix or delete it", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(err).with_context(|| format!("Cannot read {}", path.display())),
    }
}

// writes to a temp file first so a crash never leaves half a file
pub fn write<T: Serialize>(name: &str, value: &T) -> anyhow::Result<()> {
    let path = path(name).context("No config folder on this system")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string_pretty(value)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let config = Config {
            mode: "custom".into(),
            custom_folder: Some("C:\\sounds".into()),
            sensitivity: 7.5,
            ..Config::default()
        };
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.mode, "custom");
        assert_eq!(back.custom_folder.as_deref(), Some("C:\\sounds"));
        assert_eq!(back.sensitivity, 7.5);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let config: Config = toml::from_str("sensitivity = 3.0\nsource = \"mic\"").unwrap();
        assert_eq!(config.sensitivity, 3.0);
        assert_eq!(config.source, SourceMode::Mic);
        assert_eq!(config.mode, "pain");
        assert_eq!(config.cooldown_ms, 450);
    }
}
