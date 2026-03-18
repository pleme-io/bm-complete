use anyhow::Result;
use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Trait abstracting the configuration surface needed by the completion engine.
pub trait CompletionConfig {
    /// Maximum number of results to return per query.
    fn max_results(&self) -> usize;
    /// Whether to include filesystem path completions.
    fn index_path(&self) -> bool;
    /// Directories to scan for fish completion files.
    fn fish_completion_dirs(&self) -> &[String];
    /// Base directory for cache artifacts.
    fn cache_dir(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,
    #[serde(default)]
    pub fish_completion_dirs: Vec<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_true")]
    pub index_man_pages: bool,
    #[serde(default = "default_true")]
    pub index_help_flags: bool,
    #[serde(default = "default_true")]
    pub index_path: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_dir: default_cache_dir(),
            fish_completion_dirs: vec![
                "/usr/share/fish/completions".into(),
                "/usr/local/share/fish/completions".into(),
            ],
            max_results: 50,
            index_man_pages: true,
            index_help_flags: true,
            index_path: true,
        }
    }
}

impl CompletionConfig for Config {
    fn max_results(&self) -> usize {
        self.max_results
    }

    fn index_path(&self) -> bool {
        self.index_path
    }

    fn fish_completion_dirs(&self) -> &[String] {
        &self.fish_completion_dirs
    }

    fn cache_dir(&self) -> &str {
        &self.cache_dir
    }
}

fn default_cache_dir() -> String {
    dirs::cache_dir()
        .map(|d| d.join("bm-complete").to_string_lossy().into_owned())
        .unwrap_or_else(|| "$HOME/.cache/bm-complete".into())
}

fn default_max_results() -> usize {
    50
}
fn default_true() -> bool {
    true
}

pub fn load(path: Option<&Path>) -> Result<Config> {
    let mut figment = Figment::new();
    if let Some(p) = path {
        figment = figment.merge(Yaml::file(p));
    }
    figment = figment.merge(Env::prefixed("BM_COMPLETE_").split("__"));
    let config: Config = figment.extract().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(config)
}

// ═══════════════════════════════════════════════════════════════════
// Test configuration
// ═══════════════════════════════════════════════════════════════════

/// Lightweight configuration for tests — all fields are public.
pub struct TestConfig {
    pub max_results: usize,
    pub index_path: bool,
    pub fish_completion_dirs: Vec<String>,
    pub cache_dir: String,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            max_results: 50,
            index_path: true,
            fish_completion_dirs: Vec::new(),
            cache_dir: "/tmp/bm-complete-test".into(),
        }
    }
}

impl CompletionConfig for TestConfig {
    fn max_results(&self) -> usize {
        self.max_results
    }

    fn index_path(&self) -> bool {
        self.index_path
    }

    fn fish_completion_dirs(&self) -> &[String] {
        &self.fish_completion_dirs
    }

    fn cache_dir(&self) -> &str {
        &self.cache_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let cfg = TestConfig::default();
        assert_eq!(cfg.max_results(), 50);
        assert!(cfg.index_path());
        assert!(cfg.fish_completion_dirs().is_empty());
        assert_eq!(cfg.cache_dir(), "/tmp/bm-complete-test");
    }

    #[test]
    fn test_config_overrides() {
        let cfg = TestConfig {
            max_results: 10,
            index_path: false,
            fish_completion_dirs: vec!["/custom/dir".into()],
            cache_dir: "/custom/cache".into(),
        };
        assert_eq!(cfg.max_results(), 10);
        assert!(!cfg.index_path());
        assert_eq!(cfg.fish_completion_dirs(), &["/custom/dir".to_string()]);
        assert_eq!(cfg.cache_dir(), "/custom/cache");
    }
}
