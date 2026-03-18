use anyhow::Result;
use crate::store::CompletionEntry;
use std::path::{Path, PathBuf};

/// Pluggable completion source — each implementation knows how to
/// extract completions from one system (fish, man, --help, etc.).
pub trait CompletionSource {
    /// Human-readable name for this source.
    fn name(&self) -> &str;
    /// Extract all completion entries from this source.
    fn entries(&self) -> Result<Vec<CompletionEntry>>;
}

/// Fish shell completion files source.
pub struct FishSource {
    pub dirs: Vec<PathBuf>,
}

impl FishSource {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    /// Default fish completion directories.
    pub fn default_dirs() -> Vec<PathBuf> {
        [
            "/usr/share/fish/completions",
            "/usr/local/share/fish/completions",
            "/opt/homebrew/share/fish/completions",
        ]
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
    }

    fn parse_file(path: &Path) -> Vec<CompletionEntry> {
        let command = match path.file_stem().and_then(|s| s.to_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return Vec::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut entries = Vec::new();
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
                        completion = format!("-{}", parts[i + 1]);
                        i += 2;
                    }
                    "-l" if i + 1 < parts.len() => {
                        completion = format!("--{}", parts[i + 1]);
                        i += 2;
                    }
                    "-a" if i + 1 < parts.len() => {
                        let val = parts[i + 1].trim_matches('\'').trim_matches('"');
                        for v in val.split_whitespace() {
                            entries.push(CompletionEntry {
                                command: command.clone(),
                                completion: v.to_string(),
                                description: String::new(),
                                source: "fish".into(),
                            });
                        }
                        i += 2;
                    }
                    "-d" if i + 1 < parts.len() => {
                        let rest = parts[i + 1..].join(" ");
                        if rest.starts_with('\'') || rest.starts_with('"') {
                            let quote = rest.chars().next().unwrap();
                            if let Some(end) = rest[1..].find(quote) {
                                description = rest[1..end + 1].to_string();
                            }
                        } else {
                            description = parts[i + 1].to_string();
                        }
                        i = parts.len();
                    }
                    _ => {
                        i += 1;
                    }
                }
            }

            if !completion.is_empty() {
                entries.push(CompletionEntry {
                    command: command.clone(),
                    completion,
                    description,
                    source: "fish".into(),
                });
            }
        }

        entries
    }
}

impl CompletionSource for FishSource {
    fn name(&self) -> &str {
        "fish"
    }

    fn entries(&self) -> Result<Vec<CompletionEntry>> {
        let mut all = Vec::new();
        for dir in &self.dirs {
            if !dir.is_dir() {
                continue;
            }
            let entries = std::fs::read_dir(dir)?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("fish") {
                    all.extend(Self::parse_file(&path));
                }
            }
        }
        Ok(all)
    }
}

/// Mock completion source for testing.
pub struct MockSource {
    pub name: String,
    pub data: Vec<CompletionEntry>,
}

impl CompletionSource for MockSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn entries(&self) -> Result<Vec<CompletionEntry>> {
        Ok(self.data.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn mock_source_returns_data() {
        let source = MockSource {
            name: "test".into(),
            data: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "mock".into(),
            }],
        };
        assert_eq!(source.name(), "test");
        let entries = source.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].completion, "commit");
    }

    #[test]
    fn fish_source_with_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("git.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c git -l commit -d 'Record changes'").unwrap();
        writeln!(f, "complete -c git -l push -d 'Update remote'").unwrap();

        let source = FishSource::new(vec![dir.path().to_path_buf()]);
        let entries = source.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.completion == "--commit"));
        assert!(entries.iter().any(|e| e.completion == "--push"));
    }

    #[test]
    fn fish_source_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let source = FishSource::new(vec![dir.path().to_path_buf()]);
        let entries = source.entries().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn fish_source_nonexistent_dir() {
        let source = FishSource::new(vec![PathBuf::from("/tmp/nonexistent-bm-complete-test-dir")]);
        let entries = source.entries().unwrap();
        assert!(entries.is_empty());
    }
}
