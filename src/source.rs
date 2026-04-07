use anyhow::Result;
use crate::store::CompletionEntry;
use std::path::{Path, PathBuf};

/// Pluggable completion source — each implementation knows how to
/// extract completions from one system (fish, man, --help, etc.).
pub trait CompletionSource: Send + Sync {
    /// Human-readable name for this source.
    fn name(&self) -> &str;
    /// Extract all completion entries from this source.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot be read (e.g. I/O failure).
    fn entries(&self) -> Result<Vec<CompletionEntry>>;
}

/// Fish shell completion files source.
#[derive(Debug, PartialEq)]
pub struct FishSource {
    pub dirs: Vec<PathBuf>,
}

impl FishSource {
    #[must_use]
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    /// Default fish completion directories.
    #[must_use]
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

    #[must_use]
    fn parse_file(path: &Path) -> Vec<CompletionEntry> {
        let command = match path.file_stem().and_then(|s| s.to_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return Vec::new(),
        };

        let Ok(content) = std::fs::read_to_string(path) else {
            return Vec::new();
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
                        if let Some(quote @ ('\'' | '"')) = rest.chars().next() {
                            if let Some(end) = rest[1..].find(quote) {
                                description = rest[1..=end].to_string();
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

impl Default for FishSource {
    fn default() -> Self {
        Self::new(Self::default_dirs())
    }
}

impl CompletionSource for FishSource {
    fn name(&self) -> &'static str {
        "fish"
    }

    fn entries(&self) -> Result<Vec<CompletionEntry>> {
        let all = self
            .dirs
            .iter()
            .filter(|d| d.is_dir())
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten())
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("fish"))
            .flat_map(|e| Self::parse_file(&e.path()))
            .collect();
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

    // ── parse_file edge cases ────────────────────────────────────

    #[test]
    fn fish_parse_short_flags() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("ls.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c ls -s l -d 'Long listing'").unwrap();
        writeln!(f, "complete -c ls -s a -d 'Show hidden'").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.completion == "-l"));
        assert!(entries.iter().any(|e| e.completion == "-a"));
    }

    #[test]
    fn fish_parse_argument_completions_single() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("git.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c git -a commit").unwrap();
        writeln!(f, "complete -c git -a push").unwrap();
        writeln!(f, "complete -c git -a pull").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.completion == "commit"));
        assert!(entries.iter().any(|e| e.completion == "push"));
        assert!(entries.iter().any(|e| e.completion == "pull"));
    }

    #[test]
    fn fish_parse_argument_quoted_single_word() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("git.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c git -a 'status'").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(
            entries.iter().any(|e| e.completion == "status"),
            "quoted single-word -a should produce an entry: {entries:?}"
        );
    }

    #[test]
    fn fish_parse_description_with_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("git.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c git -l commit -d 'Record changes to repo'").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(!entries.is_empty());
        let commit = entries.iter().find(|e| e.completion == "--commit").unwrap();
        assert_eq!(commit.description, "Record changes to repo");
    }

    #[test]
    fn fish_parse_description_without_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("test.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c test -l verbose -d verbose-mode").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(!entries.is_empty());
        let entry = entries.iter().find(|e| e.completion == "--verbose").unwrap();
        assert_eq!(entry.description, "verbose-mode");
    }

    #[test]
    fn fish_parse_skips_non_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("git.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "# This is a comment").unwrap();
        writeln!(f, "set -l commands commit push").unwrap();
        writeln!(f, "complete -c git -l commit").unwrap();
        writeln!(f).unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].completion, "--commit");
    }

    #[test]
    fn fish_parse_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("empty.fish");
        std::fs::write(&fish_file, "").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(entries.is_empty());
    }

    #[test]
    fn fish_parse_unreadable_file() {
        let entries = FishSource::parse_file(Path::new("/nonexistent/file.fish"));
        assert!(entries.is_empty());
    }

    #[test]
    fn fish_parse_file_dotfish_uses_stem_as_command() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join(".fish");
        std::fs::write(&fish_file, "complete -c test -l opt").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(
            entries.iter().all(|e| e.command == ".fish"),
            "file_stem of '.fish' is '.fish' in Rust, used as command name"
        );
    }

    #[test]
    fn fish_source_skips_non_fish_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("git.fish"),
            "complete -c git -l commit\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a fish file\n").unwrap();
        std::fs::write(dir.path().join("script.sh"), "#!/bin/bash\n").unwrap();

        let source = FishSource::new(vec![dir.path().to_path_buf()]);
        let entries = source.entries().unwrap();
        assert!(
            entries.iter().all(|e| e.source == "fish"),
            "all entries should come from .fish files"
        );
        assert!(entries.iter().any(|e| e.completion == "--commit"));
    }

    #[test]
    fn fish_source_multiple_dirs() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let mut f = std::fs::File::create(dir1.path().join("git.fish")).unwrap();
        writeln!(f, "complete -c git -l commit").unwrap();

        let mut f = std::fs::File::create(dir2.path().join("cargo.fish")).unwrap();
        writeln!(f, "complete -c cargo -l build").unwrap();

        let source =
            FishSource::new(vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()]);
        let entries = source.entries().unwrap();
        assert!(entries.iter().any(|e| e.command == "git"));
        assert!(entries.iter().any(|e| e.command == "cargo"));
    }

    #[test]
    fn fish_source_command_derived_from_filename() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("kubectl.fish")).unwrap();
        writeln!(f, "complete -c kubectl -l get").unwrap();

        let source = FishSource::new(vec![dir.path().to_path_buf()]);
        let entries = source.entries().unwrap();
        assert!(entries.iter().all(|e| e.command == "kubectl"));
    }

    #[test]
    fn fish_source_default_dirs_returns_existing_only() {
        let dirs = FishSource::default_dirs();
        for dir in &dirs {
            assert!(dir.exists(), "default_dirs should only return existing paths");
        }
    }

    #[test]
    fn fish_source_new_and_default_equivalence() {
        let default = FishSource::default();
        let manual = FishSource::new(FishSource::default_dirs());
        assert_eq!(default, manual);
    }

    #[test]
    fn fish_source_name_returns_fish() {
        let source = FishSource::new(vec![]);
        assert_eq!(source.name(), "fish");
    }

    #[test]
    fn mock_source_empty_data() {
        let source = MockSource {
            name: "empty".into(),
            data: vec![],
        };
        assert_eq!(source.name(), "empty");
        assert!(source.entries().unwrap().is_empty());
    }

    #[test]
    fn fish_parse_mixed_flags_and_args() {
        let dir = tempfile::tempdir().unwrap();
        let fish_file = dir.path().join("docker.fish");
        let mut f = std::fs::File::create(&fish_file).unwrap();
        writeln!(f, "complete -c docker -s v -d 'Version'").unwrap();
        writeln!(f, "complete -c docker -l help -d 'Show help'").unwrap();
        writeln!(f, "complete -c docker -a run").unwrap();
        writeln!(f, "complete -c docker -a build").unwrap();
        writeln!(f, "complete -c docker -a push").unwrap();

        let entries = FishSource::parse_file(&fish_file);
        assert!(entries.iter().any(|e| e.completion == "-v"));
        assert!(entries.iter().any(|e| e.completion == "--help"));
        assert!(entries.iter().any(|e| e.completion == "run"));
        assert!(entries.iter().any(|e| e.completion == "build"));
        assert!(entries.iter().any(|e| e.completion == "push"));
    }
}
