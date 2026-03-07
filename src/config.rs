use anyhow::Result;
use figment::{
    providers::{Env, Format, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

fn default_cache_dir() -> String {
    dirs::cache_dir()
        .map(|d| d.join("bm-complete").to_string_lossy().into_owned())
        .unwrap_or_else(|| "$HOME/.cache/bm-complete".into())
}

fn default_max_results() -> usize { 50 }
fn default_true() -> bool { true }

pub fn load(path: Option<&Path>) -> Result<Config> {
    let mut figment = Figment::new();
    if let Some(p) = path {
        figment = figment.merge(Yaml::file(p));
    }
    figment = figment.merge(Env::prefixed("BM_COMPLETE_").split("__"));
    let config: Config = figment.extract().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(config)
}
