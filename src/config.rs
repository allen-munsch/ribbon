//! Belt configuration — file paths, git roots, A2A endpoints.
//!
//! Configuration is read from (in order of precedence):
//! 1. Command-line flags (--config, --log, --project-root)
//! 2. Environment variables (BELT_CONFIG, BELT_LOG_PATH, BELT_PROJECT_ROOT)
//! 3. `.belt/config.toml` — searched upward from cwd (or --project-root)
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

/// Result of config discovery — includes the config file's directory
/// for resolving relative paths (like log_path) correctly.
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub config: BeltConfig,
    /// Directory containing the config file. Used to resolve relative log_path.
    pub config_dir: Option<PathBuf>,
}

impl DiscoveredConfig {
    /// Resolve the log path relative to the config file's directory.
    /// If config_dir is None (no config file found, using defaults),
    /// falls back to the current directory.
    pub fn resolve_log_path(&self) -> PathBuf {
        if self.config.log_path.is_absolute() {
            return self.config.log_path.clone();
        }

        match &self.config_dir {
            Some(dir) => dir.join(&self.config.log_path),
            None => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                cwd.join(&self.config.log_path)
            }
        }
    }
}

impl BeltConfig {
    /// Search for and load configuration from `.belt/config.toml`.
    ///
    /// Searches upward from `search_root` (or cwd if None) to the filesystem root.
    /// Returns the config AND the directory containing the config file.
    pub fn discover_from(search_root: Option<&Path>) -> Result<DiscoveredConfig, ConfigError> {
        let config_path = find_config_file_from(search_root)?;

        match config_path {
            Some(ref path) => {
                let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
                    path: path.clone(),
                    source: e,
                })?;
                let config: BeltConfig =
                    toml::from_str(&content).map_err(|e| ConfigError::Parse {
                        path: path.clone(),
                        source: e,
                    })?;
                // config_dir = project root (parent of .belt/)
                let config_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf());
                Ok(DiscoveredConfig { config, config_dir })
            }
            None => Ok(DiscoveredConfig {
                config: BeltConfig::default(),
                config_dir: None,
            }),
        }
    }

    /// Search upward from the current directory.
    pub fn discover() -> Result<DiscoveredConfig, ConfigError> {
        Self::discover_from(None)
    }

    /// Load config from a specific path. Returns config_dir = parent of path.
    pub fn from_file(path: &Path) -> Result<DiscoveredConfig, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let config: BeltConfig = toml::from_str(&content).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            source: e,
        })?;
        // config_dir = project root (parent of .belt/)
        let config_dir = path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        Ok(DiscoveredConfig { config, config_dir })
    }

    /// Override with a specific log path.
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

/// Search for `.belt/config.toml` starting from `search_root` (or cwd) upward.
fn find_config_file_from(search_root: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    let start = match search_root {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            cwd.join(p)
        }
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut current = Some(start.as_path());

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

    #[test]
    fn test_discovered_config_resolve_log_path() {
        let config = BeltConfig::default();
        let dc = DiscoveredConfig {
            config,
            config_dir: Some(PathBuf::from("/home/user/project")),
        };
        // log_path is relative, should resolve against project root
        let resolved = dc.resolve_log_path();
        assert_eq!(resolved, PathBuf::from("/home/user/project/events.ndjson"));
    }
}
