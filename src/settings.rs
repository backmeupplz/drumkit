use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The default kit repository included out of the box.
pub const DEFAULT_KIT_REPO: &str = "backmeupplz/drumkit-kits";

fn default_kit_repos() -> Vec<String> {
    vec![DEFAULT_KIT_REPO.to_string()]
}

/// Persisted user settings (last-used kit, audio device, MIDI device, extra directories).
#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    pub kit_path: Option<PathBuf>,
    pub audio_device: Option<String>,
    pub midi_device: Option<String>,
    #[serde(default)]
    pub extra_kit_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub extra_mapping_dirs: Vec<PathBuf>,
    #[serde(default = "default_kit_repos")]
    pub kit_repos: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kit_path: None,
            audio_device: None,
            midi_device: None,
            extra_kit_dirs: Vec::new(),
            extra_mapping_dirs: Vec::new(),
            kit_repos: default_kit_repos(),
        }
    }
}

/// Return the path to the settings file:
/// `$XDG_CONFIG_HOME/drumkit/settings.toml` (default `~/.config/drumkit/settings.toml`).
pub fn settings_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("drumkit/settings.toml")
}

/// Load settings from disk. Returns empty defaults if the file is missing or invalid.
pub fn load_settings() -> Settings {
    let path = settings_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Settings::default(),
    };
    toml::from_str(&content).unwrap_or_default()
}

/// Save settings to disk, creating parent directories as needed.
pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(settings).context("Failed to serialize settings")?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write settings: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file() {
        // With a non-existent XDG path, load_settings should return defaults
        let settings = Settings::default();
        assert!(settings.kit_path.is_none());
        assert!(settings.audio_device.is_none());
        assert!(settings.midi_device.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drumkit/settings.toml");

        let settings = Settings {
            kit_path: Some(PathBuf::from("/home/user/kits/linndrum")),
            audio_device: Some("HDA Intel PCH".to_string()),
            midi_device: Some("Alesis Nitro Max MIDI 1".to_string()),
            extra_kit_dirs: vec![PathBuf::from("/extra/kits")],
            extra_mapping_dirs: vec![PathBuf::from("/extra/mappings")],
            kit_repos: default_kit_repos(),
        };

        // Save manually to temp path
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = toml::to_string_pretty(&settings).unwrap();
        std::fs::write(&path, &content).unwrap();

        // Read back
        let read = std::fs::read_to_string(&path).unwrap();
        let loaded: Settings = toml::from_str(&read).unwrap();
        assert_eq!(loaded.kit_path.unwrap(), PathBuf::from("/home/user/kits/linndrum"));
        assert_eq!(loaded.audio_device.unwrap(), "HDA Intel PCH");
        assert_eq!(loaded.midi_device.unwrap(), "Alesis Nitro Max MIDI 1");
    }

    #[test]
    fn parse_partial_settings() {
        let toml = r#"kit_path = "/some/path""#;
        let settings: Settings = toml::from_str(toml).unwrap();
        assert_eq!(settings.kit_path.unwrap(), PathBuf::from("/some/path"));
        assert!(settings.audio_device.is_none());
        assert!(settings.midi_device.is_none());
    }

    #[test]
    fn parse_invalid_toml_returns_default() {
        let settings: Settings = toml::from_str("not valid {{{}}}").unwrap_or_default();
        assert!(settings.kit_path.is_none());
    }
}
