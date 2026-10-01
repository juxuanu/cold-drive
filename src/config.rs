//! The settings kept between runs, as JSON in the platform's configuration
//! directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The CLI to run, when not the one on `PATH`.
    pub cli_path: Option<PathBuf>,
    pub view: View,
}

/// How a folder's contents are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    List,
    Grid,
}

impl View {
    pub fn toggled(self) -> Self {
        match self {
            View::List => View::Grid,
            View::Grid => View::List,
        }
    }
}

impl Config {
    /// The saved settings, or the defaults if there are none or they cannot
    /// be read.
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };

        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|error| {
                tracing::warn!(path = %path.display(), %error, "ignoring unreadable settings");
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read settings");
                Self::default()
            }
        }
    }

    pub async fn save(self) -> Result<(), String> {
        let path = path().ok_or("There is no configuration directory")?;
        let json = serde_json::to_string_pretty(&self).map_err(|error| error.to_string())?;

        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|error| error.to_string())?;
        }
        tokio::fs::write(&path, json)
            .await
            .map_err(|error| format!("Could not save settings: {error}"))?;

        tracing::debug!(path = %path.display(), "settings saved");
        Ok(())
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("cold-pass").join("config.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_their_defaults() {
        let config: Config = serde_json::from_str(r#"{"view":"grid"}"#).unwrap();

        assert_eq!(config.view, View::Grid);
        assert_eq!(config.cli_path, None);
    }
}
