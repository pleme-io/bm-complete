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
    // When buffer ends with whitespace after a word, prefix is empty (new argument)
    let prefix = if buffer.ends_with(char::is_whitespace) || words.len() <= 1 {
        ""
    } else {
        words.last().unwrap_or(&"")
    };

    let dirs_only = matches!(command, "cd" | "pushd" | "popd");

    // For directory-navigation commands, go straight to path completion
    if dirs_only && cfg.index_path {
        return Ok(path_completions(prefix, cfg.max_results, true));
    }

    // Query stored completions
    let mut results = store.query(command, prefix, cfg.max_results)?;

    // If no stored completions, try path completion
    if results.is_empty() && cfg.index_path {
        results = path_completions(prefix, cfg.max_results, false);
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

/// Path completion with proper progressive directory traversal.
///
/// Handles three prefix shapes:
///   ""        → list CWD entries, no filter
///   "src/"    → list src/ contents, no filter (trailing slash = descend)
///   "src/ma"  → list src/ contents, filter by "ma"
///   "fo"      → list CWD entries, filter by "fo"
fn path_completions(prefix: &str, limit: usize, dirs_only: bool) -> Vec<CompletionEntry> {
    // Expand leading ~ to home directory
    let expanded;
    let prefix = if prefix.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            expanded = format!("{}{}", home.display(), &prefix[1..]);
            &expanded
        } else {
            prefix
        }
    } else {
        prefix
    };

    let (dir, file_prefix, base) = if prefix.ends_with('/') {
        // Trailing slash: list this directory's contents, no filter
        (Path::new(prefix).to_path_buf(), "", prefix)
    } else if prefix.contains('/') {
        // Mid-path: parent is the directory, filename portion is the filter
        let slash = prefix.rfind('/').unwrap();
        let dir_part = &prefix[..=slash];
        let name_part = &prefix[slash + 1..];
        (Path::new(dir_part).to_path_buf(), name_part, dir_part)
    } else {
        // No slashes: list CWD, filter by prefix
        (Path::new(".").to_path_buf(), prefix, "")
    };

    let mut results = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return results;
    };

    for entry in entries.flatten() {
        if results.len() >= limit {
            break;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files unless the filter starts with '.'
        if name_str.starts_with('.') && !file_prefix.starts_with('.') {
            continue;
        }

        if !file_prefix.is_empty() && !name_str.starts_with(file_prefix) {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if dirs_only && !is_dir {
            continue;
        }

        let suffix = if is_dir { "/" } else { "" };
        let completion = format!("{base}{name_str}{suffix}");

        results.push(CompletionEntry {
            command: String::new(),
            completion,
            description: if is_dir { "directory" } else { "file" }.into(),
            source: "path".into(),
        });
    }

    results.sort_by(|a, b| a.completion.cmp(&b.completion));
    results
}
