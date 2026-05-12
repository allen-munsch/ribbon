//! Belt configuration — file paths, git roots, A2A endpoints.
//!
//! Configuration is read from (in order of precedence):
//! 1. Command-line flags
//! 2. Environment variables (BELT_*)
//! 3. `.belt/config.toml` in the current directory or ancestor
//! 4. Built-in defaults

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level belt configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeltConfig {
    /// Path to the ndjson event log (relative to config file or absolute).
    #[serde(default = "default_log_path")]
    pub log_path: PathBuf,

    /// Known agents (for validation and status).
    #[serde(default)]
    pub agents: Vec<String>,

    /// Git roots for commit verification.
    /// Maps agent name → path to git repository.
    #[serde(default)]
    pub git_roots: HashMap<String, PathBuf>,

    /// Optional A2A endpoint for bridging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a2a_url: Option<String>,

    /// Remote name for git verification.
    #[serde(default = "default_remote")]
    pub git_remote: String,

    /// Branch name for git verification.
    #[serde(default = "default_branch")]
    pub git_branch: String,

    /// Timezone for rendering (default: UTC).
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_log_path() -> PathBuf {
    PathBuf::from("events.ndjson")
}

fn default_remote() -> String {
    "origin".to_string()
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_timezone() -> String {
    "UTC".to_string()
}

impl Default for BeltConfig {
    fn default() -> Self {
        BeltConfig {
            log_path: default_log_path(),
            agents: Vec::new(),
            git_roots: HashMap::new(),
            a2a_url: None,
            git_remote: default_remote(),
            git_branch: default_branch(),
            timezone: default_timezone(),
        }
    }
}

impl BeltConfig {
    /// Search for and load configuration from `.belt/config.toml`.
    ///
    /// Searches upward from the current directory to the filesystem root.
    /// Returns default config if no config file is found.
    pub fn discover() -> Result<Self, ConfigError> {
        let config_path = find_config_file()?;

        match config_path {
            Some(path) => {
                let content = std::fs::read_to_string(&path).map_err(|e| ConfigError::Io {
                    path: path.clone(),
                    source: e,
                })?;
                let config: BeltConfig =
                    toml::from_str(&content).map_err(|e| ConfigError::Parse {
                        path: path.clone(),
                        source: e,
                    })?;
                Ok(config)
            }
            None => Ok(BeltConfig::default()),
        }
    }

    /// Load config from a specific path.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Resolve log_path relative to the config file's directory.
    pub fn resolve_log_path(&self, config_dir: Option<&Path>) -> PathBuf {
        if self.log_path.is_absolute() {
            return self.log_path.clone();
        }

        match config_dir {
            Some(dir) => dir.join(&self.log_path),
            None => {
                // Try relative to current directory
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                cwd.join(&self.log_path)
            }
        }
    }

    /// Override with a specific log path from CLI.
    pub fn with_log_path(mut self, path: PathBuf) -> Self {
        self.log_path = path;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

/// Search for `.belt/config.toml` starting from cwd upward.
fn find_config_file() -> Result<Option<PathBuf>, ConfigError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut current = Some(cwd.as_path());

    while let Some(dir) = current {
        let candidate = dir.join(".belt").join("config.toml");
        if candidate.exists() {
            return Ok(Some(candidate));
        }
        current = dir.parent();
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BeltConfig::default();
        assert_eq!(config.log_path, PathBuf::from("events.ndjson"));
        assert_eq!(config.git_remote, "origin");
        assert_eq!(config.git_branch, "main");
    }
}
