//! Path completion — directory listing, tilde expansion, filtering.
//!
//! Extracted from the main completions module for cohesion.

use crate::store::CompletionEntry;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A single directory entry returned by [`PathProvider::list_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Abstraction over filesystem operations needed by path completion.
///
/// Implementations provide directory listing, existence checks, and home
/// directory resolution. The trait is object-safe for dynamic dispatch.
pub trait PathProvider: Send + Sync {
    /// List entries in a directory. Returns an error if the directory
    /// cannot be read (does not exist, permission denied, etc.).
    fn list_dir(&self, dir: &Path) -> Result<Vec<DirEntry>>;
    /// Check whether a path exists.
    fn exists(&self, path: &Path) -> bool;
    /// Check whether a path is a directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// Return the user's home directory, if known.
    fn home_dir(&self) -> Option<PathBuf>;
}

/// Real filesystem [`PathProvider`].
pub struct FsPathProvider;

impl PathProvider for FsPathProvider {
    fn list_dir(&self, dir: &Path) -> Result<Vec<DirEntry>> {
        let rd = std::fs::read_dir(dir)?;
        let mut entries = Vec::new();
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(DirEntry { name, is_dir });
        }
        Ok(entries)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }
}

/// In-memory [`PathProvider`] for tests.
#[cfg(test)]
pub struct MockPathProvider {
    pub entries: std::collections::HashMap<PathBuf, Vec<DirEntry>>,
    pub home: Option<PathBuf>,
}

#[cfg(test)]
impl PathProvider for MockPathProvider {
    fn list_dir(&self, dir: &Path) -> Result<Vec<DirEntry>> {
        self.entries
            .get(dir)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no such directory: {}", dir.display()))
    }

    fn exists(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// Path completion with proper progressive directory traversal.
///
/// Handles three prefix shapes:
///   ""        -> list CWD entries, no filter
///   "src/"    -> list src/ contents, no filter (trailing slash = descend)
///   "src/ma"  -> list src/ contents, filter by "ma"
///   "fo"      -> list CWD entries, filter by "fo"
pub(crate) fn path_completions(
    prefix: &str,
    limit: usize,
    dirs_only: bool,
    paths: &dyn PathProvider,
) -> Vec<CompletionEntry> {
    let expanded;
    let prefix = if prefix.starts_with('~') {
        if let Some(home) = paths.home_dir() {
            expanded = format!("{}{}", home.display(), &prefix[1..]);
            &expanded
        } else {
            prefix
        }
    } else {
        prefix
    };

    let (dir, file_prefix, base) = if prefix.ends_with('/') {
        (Path::new(prefix).to_path_buf(), "", prefix)
    } else if let Some(slash) = prefix.rfind('/') {
        let dir_part = &prefix[..=slash];
        let name_part = &prefix[slash + 1..];
        (Path::new(dir_part).to_path_buf(), name_part, dir_part)
    } else {
        (Path::new(".").to_path_buf(), prefix, "")
    };

    let Ok(entries) = paths.list_dir(&dir) else {
        return Vec::new();
    };

    let mut results: Vec<CompletionEntry> = entries
        .iter()
        .filter(|e| !e.name.starts_with('.') || file_prefix.starts_with('.'))
        .filter(|e| file_prefix.is_empty() || e.name.starts_with(file_prefix))
        .filter(|e| !dirs_only || e.is_dir)
        .take(limit)
        .map(|e| {
            let suffix = if e.is_dir { "/" } else { "" };
            CompletionEntry {
                command: String::new(),
                completion: format!("{base}{}{suffix}", e.name),
                description: if e.is_dir { "directory" } else { "file" }.into(),
                source: "path".into(),
            }
        })
        .collect();

    results.sort_by(|a, b| a.completion.cmp(&b.completion));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fs_paths() -> FsPathProvider {
        FsPathProvider
    }

    #[test]
    fn path_completions_returns_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "content").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let prefix = format!("{}/", dir.path().display());
        let results = path_completions(&prefix, 50, false, &fs_paths());

        assert!(
            results.iter().any(|r| r.completion.ends_with("subdir/")),
            "should contain subdir/ entry: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.ends_with("hello.txt")),
            "should contain hello.txt entry: {results:?}"
        );

        let dir_entry = results
            .iter()
            .find(|r| r.completion.contains("subdir"))
            .expect("subdir entry should exist");
        assert!(dir_entry.completion.ends_with('/'));
        assert_eq!(dir_entry.description, "directory");

        let file_entry = results
            .iter()
            .find(|r| r.completion.contains("hello.txt"))
            .expect("hello.txt entry should exist");
        assert!(!file_entry.completion.ends_with('/'));
        assert_eq!(file_entry.description, "file");
    }

    #[test]
    fn path_completions_hidden_filtered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), "secret").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "public").unwrap();

        let prefix = format!("{}/", dir.path().display());
        let results = path_completions(&prefix, 50, false, &fs_paths());
        assert!(
            !results.iter().any(|r| r.completion.contains(".hidden")),
            "hidden files should be excluded: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.contains("visible.txt")),
            "visible files should be included: {results:?}"
        );

        let dot_prefix = format!("{}/.h", dir.path().display());
        let results = path_completions(&dot_prefix, 50, false, &fs_paths());
        assert!(
            results.iter().any(|r| r.completion.contains(".hidden")),
            "hidden files should be included with dot prefix: {results:?}"
        );
    }

    #[test]
    fn mock_lists_entries() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/fake/dir"),
            vec![
                DirEntry { name: "alpha.rs".into(), is_dir: false },
                DirEntry { name: "beta".into(), is_dir: true },
                DirEntry { name: "gamma.txt".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/fake/dir/", 50, false, &mock);
        assert_eq!(results.len(), 3);
        assert!(results.iter().any(|r| r.completion.contains("alpha.rs")));
        assert!(results.iter().any(|r| r.completion.contains("beta")));
        assert!(results.iter().any(|r| r.completion.contains("gamma.txt")));
    }

    #[test]
    fn mock_dirs_get_slash() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/mock"),
            vec![
                DirEntry { name: "mydir".into(), is_dir: true },
                DirEntry { name: "myfile".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/mock/", 50, false, &mock);
        let dir_entry = results
            .iter()
            .find(|r| r.completion.contains("mydir"))
            .expect("mydir entry");
        assert!(dir_entry.completion.ends_with('/'));

        let file_entry = results
            .iter()
            .find(|r| r.completion.contains("myfile"))
            .expect("myfile entry");
        assert!(!file_entry.completion.ends_with('/'));
    }

    #[test]
    fn mock_hidden_filtered() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/mock"),
            vec![
                DirEntry { name: ".secret".into(), is_dir: false },
                DirEntry { name: "public.txt".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/mock/", 50, false, &mock);
        assert!(!results.iter().any(|r| r.completion.contains(".secret")));
        assert!(results.iter().any(|r| r.completion.contains("public.txt")));

        let results = path_completions("/mock/.s", 50, false, &mock);
        assert!(results.iter().any(|r| r.completion.contains(".secret")));
    }

    #[test]
    fn dirs_only_filters_files() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![
                DirEntry { name: "dir1".into(), is_dir: true },
                DirEntry { name: "file1.txt".into(), is_dir: false },
                DirEntry { name: "dir2".into(), is_dir: true },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("", 50, true, &mock);
        assert!(results.iter().all(|r| r.description == "directory"));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn respects_limit() {
        let mut entries = HashMap::new();
        let many: Vec<DirEntry> = (0..20)
            .map(|i| DirEntry {
                name: format!("file{i:02}.txt"),
                is_dir: false,
            })
            .collect();
        entries.insert(PathBuf::from("/big"), many);
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/big/", 5, false, &mock);
        assert!(results.len() <= 5);
    }

    #[test]
    fn nonexistent_dir() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        let results = path_completions("/nonexistent/", 50, false, &mock);
        assert!(results.is_empty());
    }

    #[test]
    fn mid_path_filter() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/src/"),
            vec![
                DirEntry { name: "main.rs".into(), is_dir: false },
                DirEntry { name: "lib.rs".into(), is_dir: false },
                DirEntry { name: "mod.rs".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/src/ma", 50, false, &mock);
        assert_eq!(results.len(), 1);
        assert!(results[0].completion.contains("main.rs"));
    }

    #[test]
    fn results_sorted() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/dir"),
            vec![
                DirEntry { name: "zebra".into(), is_dir: false },
                DirEntry { name: "alpha".into(), is_dir: false },
                DirEntry { name: "middle".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/dir/", 50, false, &mock);
        let completions: Vec<&str> = results.iter().map(|r| r.completion.as_str()).collect();
        let mut sorted = completions.clone();
        sorted.sort();
        assert_eq!(completions, sorted);
    }

    #[test]
    fn source_is_path() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![DirEntry { name: "test.rs".into(), is_dir: false }],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("te", 50, false, &mock);
        assert!(results.iter().all(|r| r.source == "path"));
    }

    #[test]
    fn mock_exists() {
        let mut entries = HashMap::new();
        entries.insert(PathBuf::from("/exists"), vec![]);
        let mock = MockPathProvider { entries, home: None };

        assert!(mock.exists(Path::new("/exists")));
        assert!(!mock.exists(Path::new("/nope")));
    }

    #[test]
    fn mock_is_dir() {
        let mut entries = HashMap::new();
        entries.insert(PathBuf::from("/adir"), vec![]);
        let mock = MockPathProvider { entries, home: None };

        assert!(mock.is_dir(Path::new("/adir")));
        assert!(!mock.is_dir(Path::new("/nope")));
    }

    #[test]
    fn mock_list_dir_nonexistent() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        assert!(mock.list_dir(Path::new("/nope")).is_err());
    }

    #[test]
    fn mock_home_dir() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: Some(PathBuf::from("/home/test")),
        };
        assert_eq!(mock.home_dir(), Some(PathBuf::from("/home/test")));

        let mock_no_home = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        assert_eq!(mock_no_home.home_dir(), None);
    }
}
