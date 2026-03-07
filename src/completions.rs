use crate::config::Config;
use crate::store::{CompletionEntry, CompletionStore};
use anyhow::Result;
use std::path::Path;

/// Complete a command line at the given cursor position
pub fn complete(
    buffer: &str,
    _position: usize,
    store: &CompletionStore,
    cfg: &Config,
) -> Result<Vec<CompletionEntry>> {
    let words: Vec<&str> = buffer.split_whitespace().collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }

    let command = words[0];
    let prefix = if words.len() > 1 {
        words.last().unwrap_or(&"")
    } else {
        ""
    };

    // Query stored completions
    let mut results = store.query(command, prefix, cfg.max_results)?;

    // If no stored completions, try path completion
    if results.is_empty() && cfg.index_path {
        results = path_completions(prefix, cfg.max_results);
    }

    Ok(results)
}

/// Index completion sources into the store
pub fn index_sources(store: &CompletionStore, fish_dir: Option<&Path>) -> Result<()> {
    // Index fish completions if available
    let fish_dirs: Vec<&Path> = if let Some(dir) = fish_dir {
        vec![dir]
    } else {
        let defaults = vec![
            Path::new("/usr/share/fish/completions"),
            Path::new("/usr/local/share/fish/completions"),
            Path::new("/opt/homebrew/share/fish/completions"),
        ];
        defaults.into_iter().filter(|p| p.exists()).collect()
    };

    for dir in fish_dirs {
        index_fish_completions(store, dir)?;
    }

    let count = store.count()?;
    println!("indexed {count} completion entries");
    Ok(())
}

/// Parse fish completion files and insert into store
fn index_fish_completions(store: &CompletionStore, dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("fish") {
            continue;
        }

        let command = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if command.is_empty() {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Parse fish's `complete -c <command> -s <short> -l <long> -d <description>` lines
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with("complete") {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            let mut completion = String::new();
            let mut description = String::new();

            let mut i = 0;
            while i < parts.len() {
                match parts[i] {
                    "-s" if i + 1 < parts.len() => {
                        if !completion.is_empty() {
                            // Save the long option first, then override with short
                        }
                        completion = format!("-{}", parts[i + 1]);
                        i += 2;
                    }
                    "-l" if i + 1 < parts.len() => {
                        completion = format!("--{}", parts[i + 1]);
                        i += 2;
                    }
                    "-a" if i + 1 < parts.len() => {
                        // Argument completions (values)
                        let val = parts[i + 1].trim_matches('\'').trim_matches('"');
                        for v in val.split_whitespace() {
                            let _ = store.insert(&CompletionEntry {
                                command: command.clone(),
                                completion: v.to_string(),
                                description: String::new(),
                                source: "fish".into(),
                            });
                        }
                        i += 2;
                    }
                    "-d" if i + 1 < parts.len() => {
                        // Description may be quoted
                        let rest = parts[i + 1..].join(" ");
                        if rest.starts_with('\'') || rest.starts_with('"') {
                            let quote = rest.chars().next().unwrap();
                            if let Some(end) = rest[1..].find(quote) {
                                description = rest[1..end + 1].to_string();
                            }
                        } else {
                            description = parts[i + 1].to_string();
                        }
                        i = parts.len(); // consume rest
                    }
                    _ => {
                        i += 1;
                    }
                }
            }

            if !completion.is_empty() {
                let _ = store.insert(&CompletionEntry {
                    command: command.clone(),
                    completion,
                    description,
                    source: "fish".into(),
                });
            }
        }
    }

    Ok(())
}

/// Simple path completion from the current directory
fn path_completions(prefix: &str, limit: usize) -> Vec<CompletionEntry> {
    let dir = if prefix.contains('/') {
        Path::new(prefix)
            .parent()
            .unwrap_or(Path::new("."))
    } else {
        Path::new(".")
    };

    let file_prefix = if prefix.contains('/') {
        Path::new(prefix)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    } else {
        prefix
    };

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten().take(limit * 2) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(file_prefix) || file_prefix.is_empty() {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let completion = if prefix.contains('/') {
                    format!(
                        "{}/{}{}",
                        dir.display(),
                        name_str,
                        if is_dir { "/" } else { "" }
                    )
                } else {
                    format!("{}{}", name_str, if is_dir { "/" } else { "" })
                };
                results.push(CompletionEntry {
                    command: String::new(),
                    completion,
                    description: if is_dir { "directory" } else { "file" }.into(),
                    source: "path".into(),
                });
                if results.len() >= limit {
                    break;
                }
            }
        }
    }
    results
}
