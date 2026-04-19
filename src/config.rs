use anyhow::Result;
use crate::error::BmError;
use serde::{Deserialize, Serialize};
use shikumi::ProviderChain;
use std::path::Path;

/// Trait abstracting the configuration surface needed by the completion engine.
pub trait CompletionConfig: Send + Sync {
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

#[must_use]
fn default_cache_dir() -> String {
    dirs::cache_dir().map_or_else(
        || "$HOME/.cache/bm-complete".into(),
        |d| d.join("bm-complete").to_string_lossy().into_owned(),
    )
}

#[must_use]
fn default_max_results() -> usize {
    50
}
#[must_use]
fn default_true() -> bool {
    true
}

/// Load configuration from an optional YAML file path, merging with
/// environment variables prefixed with `BM_COMPLETE_`.
///
/// Delegates to shikumi's `ProviderChain` for figment layering — keeps the
/// `__` nested-key separator and the same YAML/env precedence (file wins
/// over env, matching the pre-shikumi behaviour this function had).
///
/// # Errors
///
/// Returns an error if the YAML file is malformed or extraction fails.
pub fn load(path: Option<&Path>) -> Result<Config> {
    let mut chain = ProviderChain::new().with_env("BM_COMPLETE_");
    if let Some(p) = path {
        chain = chain.with_file(p);
    }
    let config: Config = chain
        .extract()
        .map_err(|e| BmError::Config(e.to_string()))?;
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

    // ── Config (production) tests ────────────────────────────────

    #[test]
    fn config_default_values() {
        let cfg = Config::default();
        assert_eq!(cfg.max_results, 50);
        assert!(cfg.index_path);
        assert!(cfg.index_man_pages);
        assert!(cfg.index_help_flags);
        assert_eq!(cfg.fish_completion_dirs.len(), 2);
        assert!(cfg.fish_completion_dirs.contains(&"/usr/share/fish/completions".to_string()));
        assert!(cfg
            .fish_completion_dirs
            .contains(&"/usr/local/share/fish/completions".to_string()));
    }

    #[test]
    fn config_trait_delegates_correctly() {
        let cfg = Config::default();
        assert_eq!(cfg.max_results(), cfg.max_results);
        assert_eq!(cfg.index_path(), cfg.index_path);
        assert_eq!(cfg.fish_completion_dirs(), cfg.fish_completion_dirs.as_slice());
        assert_eq!(cfg.cache_dir(), cfg.cache_dir.as_str());
    }

    #[test]
    fn load_with_no_file_returns_defaults() {
        let cfg = load(None).unwrap();
        assert_eq!(cfg.max_results, 50);
        assert!(cfg.index_path);
    }

    #[test]
    fn load_with_yaml_file() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("config.yaml");
        std::fs::write(
            &yaml_path,
            "max_results: 25\nindex_path: false\nfish_completion_dirs:\n  - /custom/fish\n",
        )
        .unwrap();

        let cfg = load(Some(&yaml_path)).unwrap();
        assert_eq!(cfg.max_results, 25);
        assert!(!cfg.index_path);
        assert_eq!(cfg.fish_completion_dirs, vec!["/custom/fish".to_string()]);
    }

    #[test]
    fn load_with_nonexistent_file_returns_defaults() {
        let cfg = load(Some(std::path::Path::new("/nonexistent/config.yaml"))).unwrap();
        assert_eq!(cfg.max_results, 50);
    }

    #[test]
    fn load_with_partial_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("config.yaml");
        std::fs::write(&yaml_path, "max_results: 10\n").unwrap();

        let cfg = load(Some(&yaml_path)).unwrap();
        assert_eq!(cfg.max_results, 10);
        assert!(cfg.index_path, "unset fields should use defaults");
        assert!(cfg.index_man_pages, "unset fields should use defaults");
    }

    #[test]
    fn config_serde_roundtrip() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.max_results, cfg.max_results);
        assert_eq!(deserialized.index_path, cfg.index_path);
        assert_eq!(deserialized.cache_dir, cfg.cache_dir);
    }
}
