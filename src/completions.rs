use crate::cache::{CacheStore, Fingerprinter, resolve_completions};
use crate::config::CompletionConfig;
use crate::source::CompletionSource;
use crate::store::{CompletionEntry, Store};
use anyhow::Result;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

// ═══════════════════════════════════════════════════════════════════
// PathProvider trait — abstracts filesystem I/O for path completions
// ═══════════════════════════════════════════════════════════════════

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
    /// List entries in a directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read (does not exist,
    /// permission denied, etc.).
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

/// Context classification for completion behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionContext {
    /// cd, pushd, popd, z — directory-only results
    DirectoryNav,
    /// Prefix looks like a path (starts with /, ~, ./)
    PathCompletion,
    /// Prefix starts with - (flag completion)
    FlagCompletion,
    /// General command argument
    CommandArg,
}

impl fmt::Display for CompletionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryNav => f.write_str("directory-nav"),
            Self::PathCompletion => f.write_str("path-completion"),
            Self::FlagCompletion => f.write_str("flag-completion"),
            Self::CommandArg => f.write_str("command-arg"),
        }
    }
}

impl FromStr for CompletionContext {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "directory-nav" => Ok(Self::DirectoryNav),
            "path-completion" => Ok(Self::PathCompletion),
            "flag-completion" => Ok(Self::FlagCompletion),
            "command-arg" => Ok(Self::CommandArg),
            other => Err(format!("unknown completion context: {other}")),
        }
    }
}

/// O(1) set of commands that navigate directories.
static DIR_NAV_COMMANDS: std::sync::LazyLock<HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        ["cd", "pushd", "popd", "z", "zoxide", "j", "autojump"]
            .into_iter()
            .collect()
    });

/// Classify the completion context for a command + prefix.
#[must_use]
pub fn classify_context(command: &str, prefix: &str) -> CompletionContext {
    // Tier 1: O(1) directory navigation commands
    if DIR_NAV_COMMANDS.contains(command) {
        return CompletionContext::DirectoryNav;
    }
    // Tier 2: path shape
    if prefix.starts_with('/')
        || prefix.starts_with('~')
        || prefix.starts_with("./")
        || prefix.starts_with("../")
    {
        return CompletionContext::PathCompletion;
    }
    // Tier 3: flag shape
    if prefix.starts_with('-') {
        return CompletionContext::FlagCompletion;
    }
    CompletionContext::CommandArg
}

/// Complete a command line at the given cursor position.
///
/// # Errors
///
/// Returns an error if the backing store query fails.
pub fn complete(
    buffer: &str,
    _position: usize,
    store: &dyn Store,
    cfg: &dyn CompletionConfig,
    paths: &dyn PathProvider,
) -> Result<Vec<CompletionEntry>> {
    let words: Vec<&str> = buffer.split_whitespace().collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }

    let command = words[0];
    let prefix = if buffer.ends_with(char::is_whitespace) || words.len() <= 1 {
        ""
    } else {
        words.last().copied().unwrap_or("")
    };

    let ctx = classify_context(command, prefix);

    // Directory navigation -> path completion (dirs only)
    if ctx == CompletionContext::DirectoryNav && cfg.index_path() {
        return Ok(path_completions(prefix, cfg.max_results(), true, paths));
    }

    // Path-shaped prefix -> path completion (files + dirs)
    if ctx == CompletionContext::PathCompletion && cfg.index_path() {
        return Ok(path_completions(prefix, cfg.max_results(), false, paths));
    }

    // Query stored completions
    let mut results = store.query(command, prefix, cfg.max_results())?;

    // If no stored completions, try path completion
    if results.is_empty() && cfg.index_path() {
        results = path_completions(prefix, cfg.max_results(), false, paths);
    }

    Ok(results)
}

/// Index all given completion sources into the store.
///
/// # Errors
///
/// Returns an error if a source fails to produce entries or if store
/// insertion fails.
pub fn index_sources(store: &dyn Store, sources: &[&dyn CompletionSource]) -> Result<()> {
    for source in sources {
        let entries = source.entries()?;
        for entry in &entries {
            store.insert(entry)?;
        }
    }
    let count = store.count()?;
    println!("indexed {count} completion entries");
    Ok(())
}

/// Cache-aware variant of [`index_sources`]. Uses `resolve_cached()` and
/// stores the resolved entries into the provided store.
///
/// # Errors
///
/// Returns an error if cache resolution, source indexing, or store
/// insertion fails.
pub fn index_sources_cached(
    store: &dyn Store,
    sources: &[&dyn CompletionSource],
    cache: &dyn CacheStore<Vec<CompletionEntry>>,
    fp: &dyn Fingerprinter,
) -> Result<()> {
    let entries = resolve_completions(cache, fp, || {
        let mut all = Vec::new();
        for source in sources {
            all.extend(source.entries()?);
        }
        Ok(all)
    })?;
    for entry in &entries {
        store.insert(entry)?;
    }
    let count = store.count()?;
    println!("indexed {count} completion entries (cached)");
    Ok(())
}

/// Path completion with proper progressive directory traversal.
///
/// Handles three prefix shapes:
///   ""        -> list CWD entries, no filter
///   "src/"    -> list src/ contents, no filter (trailing slash = descend)
///   "src/ma"  -> list src/ contents, filter by "ma"
///   "fo"      -> list CWD entries, filter by "fo"
fn path_completions(
    prefix: &str,
    limit: usize,
    dirs_only: bool,
    paths: &dyn PathProvider,
) -> Vec<CompletionEntry> {
    // Expand leading ~ to home directory
    let expanded;
    let prefix = if let Some(rest) = prefix.strip_prefix('~') {
        if let Some(home) = paths.home_dir() {
            expanded = format!("{}{rest}", home.display());
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
    } else if let Some(slash) = prefix.rfind('/') {
        let dir_part = &prefix[..=slash];
        let name_part = &prefix[slash + 1..];
        (Path::new(dir_part).to_path_buf(), name_part, dir_part)
    } else {
        // No slashes: list CWD, filter by prefix
        (Path::new(".").to_path_buf(), prefix, "")
    };

    let mut results = Vec::new();
    let Ok(entries) = paths.list_dir(&dir) else {
        return results;
    };

    for entry in &entries {
        if results.len() >= limit {
            break;
        }
        let name_str = &entry.name;

        // Skip hidden files unless the filter starts with '.'
        if name_str.starts_with('.') && !file_prefix.starts_with('.') {
            continue;
        }

        if !file_prefix.is_empty() && !name_str.starts_with(file_prefix) {
            continue;
        }

        if dirs_only && !entry.is_dir {
            continue;
        }

        let suffix = if entry.is_dir { "/" } else { "" };
        let completion = format!("{base}{name_str}{suffix}");

        results.push(CompletionEntry {
            command: String::new(),
            completion,
            description: if entry.is_dir { "directory" } else { "file" }.into(),
            source: "path".into(),
        });
    }

    results.sort_by(|a, b| a.completion.cmp(&b.completion));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TestConfig;
    use crate::source::MockSource;
    use crate::store::MemStore;
    use std::collections::HashMap;

    /// Helper: create an `FsPathProvider` for tests that still use the real FS.
    fn fs_paths() -> FsPathProvider {
        FsPathProvider
    }

    // ── classify_context tests (existing) ──────────────────────────

    #[test]
    fn classify_cd_is_directory_nav() {
        assert_eq!(
            classify_context("cd", "/ni"),
            CompletionContext::DirectoryNav
        );
    }

    #[test]
    fn classify_pushd_is_directory_nav() {
        assert_eq!(
            classify_context("pushd", ""),
            CompletionContext::DirectoryNav
        );
    }

    #[test]
    fn classify_z_is_directory_nav() {
        assert_eq!(
            classify_context("z", "foo"),
            CompletionContext::DirectoryNav
        );
    }

    #[test]
    fn classify_path_prefix() {
        assert_eq!(
            classify_context("ls", "/etc"),
            CompletionContext::PathCompletion
        );
        assert_eq!(
            classify_context("cat", "~/"),
            CompletionContext::PathCompletion
        );
        assert_eq!(
            classify_context("vim", "./src"),
            CompletionContext::PathCompletion
        );
        assert_eq!(
            classify_context("rm", "../foo"),
            CompletionContext::PathCompletion
        );
    }

    #[test]
    fn classify_flag_prefix() {
        assert_eq!(
            classify_context("git", "--ver"),
            CompletionContext::FlagCompletion
        );
        assert_eq!(
            classify_context("ls", "-l"),
            CompletionContext::FlagCompletion
        );
    }

    #[test]
    fn classify_command_arg() {
        assert_eq!(
            classify_context("git", "commit"),
            CompletionContext::CommandArg
        );
        assert_eq!(
            classify_context("kubectl", "get"),
            CompletionContext::CommandArg
        );
    }

    // ── trait-based complete() tests ───────────────────────────────

    #[test]
    fn complete_empty_buffer_returns_nothing() {
        let store = MemStore::new();
        let cfg = TestConfig::default();
        let results = complete("", 0, &store, &cfg, &fs_paths()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn complete_queries_store() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "config".into(),
                description: "Get/set options".into(),
                source: "fish".into(),
            })
            .unwrap();

        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("git co", 6, &store, &cfg, &fs_paths()).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.completion == "commit"));
        assert!(results.iter().any(|r| r.completion == "config"));
    }

    #[test]
    fn complete_flag_prefix() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "--verbose".into(),
                description: "Be verbose".into(),
                source: "fish".into(),
            })
            .unwrap();

        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("git --ver", 9, &store, &cfg, &fs_paths()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "--verbose");
    }

    #[test]
    fn complete_respects_max_results() {
        let store = MemStore::new();
        for i in 0..20 {
            store
                .insert(&CompletionEntry {
                    command: "test".into(),
                    completion: format!("opt-{i:02}"),
                    description: String::new(),
                    source: "mock".into(),
                })
                .unwrap();
        }

        let cfg = TestConfig {
            max_results: 5,
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("test o", 6, &store, &cfg, &fs_paths()).unwrap();
        assert!(results.len() <= 5);
    }

    #[test]
    fn complete_no_path_when_disabled() {
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        // cd normally triggers directory nav, but with index_path=false it should
        // fall through and return nothing (no stored completions either).
        let results = complete("cd /tm", 6, &store, &cfg, &fs_paths()).unwrap();
        assert!(results.is_empty());
    }

    // ── index_sources tests ───────────────────────────────────────

    #[test]
    fn index_sources_inserts_entries() {
        let store = MemStore::new();
        let source = MockSource {
            name: "mock".into(),
            data: vec![
                CompletionEntry {
                    command: "git".into(),
                    completion: "commit".into(),
                    description: "Record changes".into(),
                    source: "mock".into(),
                },
                CompletionEntry {
                    command: "git".into(),
                    completion: "push".into(),
                    description: "Update remote".into(),
                    source: "mock".into(),
                },
            ],
        };
        index_sources(&store, &[&source as &dyn CompletionSource]).unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn index_sources_multiple_sources() {
        let store = MemStore::new();
        let s1 = MockSource {
            name: "fish".into(),
            data: vec![CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "fish".into(),
            }],
        };
        let s2 = MockSource {
            name: "man".into(),
            data: vec![CompletionEntry {
                command: "ls".into(),
                completion: "-l".into(),
                description: "Long listing".into(),
                source: "man".into(),
            }],
        };
        index_sources(
            &store,
            &[&s1 as &dyn CompletionSource, &s2 as &dyn CompletionSource],
        )
        .unwrap();
        assert_eq!(store.count().unwrap(), 2);
    }

    #[test]
    fn index_sources_empty_sources() {
        let store = MemStore::new();
        let empty: Vec<&dyn CompletionSource> = Vec::new();
        index_sources(&store, &empty).unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn index_sources_cached_populates_store() {
        use crate::cache::{FixedFingerprinter, MemCache};

        let store = MemStore::new();
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(42);
        let source = MockSource {
            name: "mock".into(),
            data: vec![CompletionEntry {
                command: "cargo".into(),
                completion: "build".into(),
                description: "Compile".into(),
                source: "mock".into(),
            }],
        };
        index_sources_cached(
            &store,
            &[&source as &dyn CompletionSource],
            &cache,
            &fp,
        )
        .unwrap();
        assert_eq!(store.count().unwrap(), 1);
        // Cache should now be populated
        assert!(cache.load().is_some());
    }

    #[test]
    fn path_completions_returns_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Create a file and a subdirectory inside the tempdir
        std::fs::write(dir.path().join("hello.txt"), "content").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let prefix = format!("{}/", dir.path().display());
        let results = path_completions(&prefix, 50, false, &fs_paths());

        // Should contain both file and directory
        assert!(
            results.iter().any(|r| r.completion.ends_with("subdir/")),
            "should contain subdir/ entry: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.ends_with("hello.txt")),
            "should contain hello.txt entry: {results:?}"
        );

        // Directory entries should have "/" suffix
        let dir_entry = results
            .iter()
            .find(|r| r.completion.contains("subdir"))
            .expect("subdir entry should exist");
        assert!(
            dir_entry.completion.ends_with('/'),
            "directory completions should end with /"
        );
        assert_eq!(dir_entry.description, "directory");

        let file_entry = results
            .iter()
            .find(|r| r.completion.contains("hello.txt"))
            .expect("hello.txt entry should exist");
        assert!(
            !file_entry.completion.ends_with('/'),
            "file completions should not end with /"
        );
        assert_eq!(file_entry.description, "file");
    }

    #[test]
    fn path_completions_hidden_filtered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), "secret").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "public").unwrap();

        // Without dot prefix — hidden files should be excluded
        let prefix = format!("{}/", dir.path().display());
        let results = path_completions(&prefix, 50, false, &fs_paths());
        assert!(
            !results.iter().any(|r| r.completion.contains(".hidden")),
            "hidden files should be excluded when prefix doesn't start with '.': {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.contains("visible.txt")),
            "visible files should be included: {results:?}"
        );

        // With dot prefix — hidden files should be included
        let dot_prefix = format!("{}/.h", dir.path().display());
        let results = path_completions(&dot_prefix, 50, false, &fs_paths());
        assert!(
            results.iter().any(|r| r.completion.contains(".hidden")),
            "hidden files should be included when prefix starts with '.': {results:?}"
        );
    }

    // ── MockPathProvider tests ────────────────────────────────────

    #[test]
    fn path_provider_mock_lists_entries() {
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
        assert_eq!(results.len(), 3, "expected 3 entries: {results:?}");
        assert!(results.iter().any(|r| r.completion.contains("alpha.rs")));
        assert!(results.iter().any(|r| r.completion.contains("beta")));
        assert!(results.iter().any(|r| r.completion.contains("gamma.txt")));
    }

    #[test]
    fn path_provider_mock_dirs_get_slash() {
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
            .expect("mydir entry should exist");
        assert!(
            dir_entry.completion.ends_with('/'),
            "directory completions should end with /: got {:?}",
            dir_entry.completion
        );

        let file_entry = results
            .iter()
            .find(|r| r.completion.contains("myfile"))
            .expect("myfile entry should exist");
        assert!(
            !file_entry.completion.ends_with('/'),
            "file completions should not end with /: got {:?}",
            file_entry.completion
        );
    }

    #[test]
    fn path_provider_mock_hidden_filtered() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/mock"),
            vec![
                DirEntry { name: ".secret".into(), is_dir: false },
                DirEntry { name: "public.txt".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };

        // Without dot prefix — hidden files should be excluded
        let results = path_completions("/mock/", 50, false, &mock);
        assert!(
            !results.iter().any(|r| r.completion.contains(".secret")),
            "hidden files should be excluded: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.contains("public.txt")),
            "visible files should be included: {results:?}"
        );

        // With dot prefix — hidden files should be included
        let results = path_completions("/mock/.s", 50, false, &mock);
        assert!(
            results.iter().any(|r| r.completion.contains(".secret")),
            "hidden files should be included when prefix starts with '.': {results:?}"
        );
    }

    // ── complete() edge cases ────────────────────────────────────

    #[test]
    fn complete_single_word_no_prefix() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();

        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("git", 3, &store, &cfg, &fs_paths()).unwrap();
        assert!(
            results.is_empty() || results.iter().all(|r| r.command == "git"),
            "single word with no space should query with empty prefix"
        );
    }

    #[test]
    fn complete_trailing_whitespace_no_prefix() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "push".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();

        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("git ", 4, &store, &cfg, &fs_paths()).unwrap();
        assert_eq!(
            results.len(),
            2,
            "trailing space should use empty prefix and return all subcommands"
        );
    }

    #[test]
    fn complete_whitespace_only_returns_empty() {
        let store = MemStore::new();
        let cfg = TestConfig::default();
        let results = complete("   ", 3, &store, &cfg, &fs_paths()).unwrap();
        assert!(results.is_empty(), "whitespace-only buffer should return nothing");
    }

    #[test]
    fn complete_directory_nav_with_mock() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![
                DirEntry { name: "src".into(), is_dir: true },
                DirEntry { name: "Cargo.toml".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider { entries, home: None };
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("cd ", 3, &store, &cfg, &mock).unwrap();
        assert!(
            results.iter().all(|r| r.description == "directory"),
            "cd should only show directories: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.completion.contains("src")),
            "should show src directory"
        );
        assert!(
            !results.iter().any(|r| r.completion.contains("Cargo.toml")),
            "should not show files in cd completion"
        );
    }

    #[test]
    fn complete_path_shaped_prefix_with_mock() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/etc"),
            vec![
                DirEntry { name: "hosts".into(), is_dir: false },
                DirEntry { name: "nginx".into(), is_dir: true },
            ],
        );
        let mock = MockPathProvider { entries, home: None };
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("cat /etc/", 9, &store, &cfg, &mock).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.completion.contains("hosts")));
        assert!(results.iter().any(|r| r.completion.contains("nginx")));
    }

    #[test]
    fn complete_tilde_expansion_with_mock() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/home/testuser"),
            vec![
                DirEntry { name: "Documents".into(), is_dir: true },
                DirEntry { name: ".bashrc".into(), is_dir: false },
            ],
        );
        let mock = MockPathProvider {
            entries,
            home: Some(PathBuf::from("/home/testuser")),
        };
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("ls ~/", 4, &store, &cfg, &mock).unwrap();
        assert!(
            results.iter().any(|r| r.completion.contains("Documents")),
            "tilde expansion should list home directory contents: {results:?}"
        );
    }

    #[test]
    fn complete_dirs_only_filters_files() {
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
        assert!(
            results.iter().all(|r| r.description == "directory"),
            "dirs_only should exclude files: {results:?}"
        );
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn path_completions_respects_limit() {
        let mut entries = HashMap::new();
        let many_entries: Vec<DirEntry> = (0..20)
            .map(|i| DirEntry {
                name: format!("file{i:02}.txt"),
                is_dir: false,
            })
            .collect();
        entries.insert(PathBuf::from("/big"), many_entries);
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("/big/", 5, false, &mock);
        assert!(
            results.len() <= 5,
            "path_completions should respect limit: got {}",
            results.len()
        );
    }

    #[test]
    fn path_completions_nonexistent_dir() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        let results = path_completions("/nonexistent/", 50, false, &mock);
        assert!(results.is_empty());
    }

    #[test]
    fn path_completions_mid_path_filter() {
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
    fn complete_fallback_to_path_when_store_empty() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![DirEntry { name: "readme.md".into(), is_dir: false }],
        );
        let mock = MockPathProvider { entries, home: None };
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("cat re", 6, &store, &cfg, &mock).unwrap();
        assert!(
            results.iter().any(|r| r.completion.contains("readme.md")),
            "should fall back to path completion when store has no matches: {results:?}"
        );
    }

    #[test]
    fn complete_no_fallback_when_store_has_matches() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![DirEntry { name: "readme.md".into(), is_dir: false }],
        );
        let mock = MockPathProvider { entries, home: None };
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "rebase".into(),
                description: "Rebase commits".into(),
                source: "fish".into(),
            })
            .unwrap();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("git re", 6, &store, &cfg, &mock).unwrap();
        assert!(
            results.iter().any(|r| r.completion == "rebase"),
            "should use store results when available"
        );
    }

    #[test]
    fn path_completions_results_sorted() {
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
        sorted.sort_unstable();
        assert_eq!(completions, sorted, "path completions should be sorted");
    }

    #[test]
    fn path_completions_source_is_path() {
        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("."),
            vec![DirEntry { name: "test.rs".into(), is_dir: false }],
        );
        let mock = MockPathProvider { entries, home: None };

        let results = path_completions("te", 50, false, &mock);
        assert!(
            results.iter().all(|r| r.source == "path"),
            "path completions should have source 'path'"
        );
    }

    #[test]
    fn classify_all_dir_nav_commands() {
        for cmd in ["cd", "pushd", "popd", "z", "zoxide", "j", "autojump"] {
            assert_eq!(
                classify_context(cmd, "anything"),
                CompletionContext::DirectoryNav,
                "{cmd} should classify as DirectoryNav"
            );
        }
    }

    #[test]
    fn classify_dot_dot_slash_is_path() {
        assert_eq!(
            classify_context("vim", "../foo"),
            CompletionContext::PathCompletion
        );
    }

    #[test]
    fn classify_single_dash_is_flag() {
        assert_eq!(
            classify_context("ls", "-"),
            CompletionContext::FlagCompletion
        );
    }

    #[test]
    fn classify_double_dash_is_flag() {
        assert_eq!(
            classify_context("git", "--"),
            CompletionContext::FlagCompletion
        );
    }

    #[test]
    fn classify_empty_prefix_is_command_arg() {
        assert_eq!(
            classify_context("git", ""),
            CompletionContext::CommandArg
        );
    }

    #[test]
    fn complete_with_index_path_disabled_skips_path_fallback() {
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: false,
            ..TestConfig::default()
        };
        let results = complete("ls /tmp", 7, &store, &cfg, &fs_paths()).unwrap();
        assert!(
            results.is_empty(),
            "with index_path=false, path-shaped prefix should not trigger path completion"
        );
    }

    #[test]
    fn mock_path_provider_exists() {
        let mut entries = HashMap::new();
        entries.insert(PathBuf::from("/exists"), vec![]);
        let mock = MockPathProvider { entries, home: None };

        assert!(mock.exists(Path::new("/exists")));
        assert!(!mock.exists(Path::new("/nope")));
    }

    #[test]
    fn mock_path_provider_is_dir() {
        let mut entries = HashMap::new();
        entries.insert(PathBuf::from("/adir"), vec![]);
        let mock = MockPathProvider { entries, home: None };

        assert!(mock.is_dir(Path::new("/adir")));
        assert!(!mock.is_dir(Path::new("/nope")));
    }

    #[test]
    fn mock_path_provider_list_dir_nonexistent() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        assert!(mock.list_dir(Path::new("/nope")).is_err());
    }

    #[test]
    fn mock_path_provider_home_dir() {
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

    #[test]
    fn context_display_fromstr_roundtrip() {
        for ctx in [
            CompletionContext::DirectoryNav,
            CompletionContext::PathCompletion,
            CompletionContext::FlagCompletion,
            CompletionContext::CommandArg,
        ] {
            let s = ctx.to_string();
            let parsed: CompletionContext = s.parse().unwrap();
            assert_eq!(parsed, ctx, "Display/FromStr round-trip failed for {ctx:?}");
        }
    }

    #[test]
    fn context_fromstr_invalid() {
        let result = "bogus".parse::<CompletionContext>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown"));
    }

    #[test]
    fn complete_tilde_without_home_dir() {
        let mock = MockPathProvider {
            entries: HashMap::new(),
            home: None,
        };
        let store = MemStore::new();
        let cfg = TestConfig {
            index_path: true,
            ..TestConfig::default()
        };

        let results = complete("ls ~/", 4, &store, &cfg, &mock).unwrap();
        assert!(
            results.is_empty(),
            "tilde without home dir should return empty: {results:?}"
        );
    }
}
