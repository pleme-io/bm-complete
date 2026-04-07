use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionEntry {
    pub command: String,
    pub completion: String,
    pub description: String,
    pub source: String, // "fish", "man", "help", "path", "custom"
}

impl Default for CompletionEntry {
    fn default() -> Self {
        Self {
            command: String::new(),
            completion: String::new(),
            description: String::new(),
            source: "custom".into(),
        }
    }
}

/// Abstraction over completion storage backends.
pub trait Store: Send + Sync {
    /// Insert (or replace) a single completion entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage backend fails.
    fn insert(&self, entry: &CompletionEntry) -> Result<()>;

    /// Query completions for `command` whose completion text starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage backend fails.
    fn query(&self, command: &str, prefix: &str, limit: usize) -> Result<Vec<CompletionEntry>>;

    /// Total number of stored entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage backend fails.
    fn count(&self) -> Result<usize>;
}

// ═══════════════════════════════════════════════════════════════════
// SQLite implementation
// ═══════════════════════════════════════════════════════════════════

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open (or create) the default database under the user cache dir.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created or the
    /// database cannot be opened.
    pub fn open_or_create() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("bm-complete");
        std::fs::create_dir_all(&cache_dir)?;
        let db_path = cache_dir.join("completions.db");
        Self::open_at(&db_path)
    }

    /// Open (or create) a database at an explicit path — useful for tests.
    ///
    /// # Errors
    ///
    /// Returns an error if the database file cannot be opened or schema
    /// creation fails.
    pub fn open_at(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).context("failed to open completion database")?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS completions (
                id INTEGER PRIMARY KEY,
                command TEXT NOT NULL,
                completion TEXT NOT NULL,
                description TEXT DEFAULT '',
                source TEXT DEFAULT 'custom',
                UNIQUE(command, completion, source)
            );
            CREATE INDEX IF NOT EXISTS idx_completions_command ON completions(command);
            CREATE INDEX IF NOT EXISTS idx_completions_prefix ON completions(completion);",
        )
        .context("failed to create tables")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl Store for SqliteStore {
    fn insert(&self, entry: &CompletionEntry) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("SqliteStore mutex poisoned: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO completions (command, completion, description, source)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                entry.command,
                entry.completion,
                entry.description,
                entry.source,
            ],
        )?;
        Ok(())
    }

    fn query(
        &self,
        command: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<CompletionEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("SqliteStore mutex poisoned: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT command, completion, description, source FROM completions
             WHERE command = ?1 AND completion LIKE ?2
             ORDER BY completion
             LIMIT ?3",
        )?;

        let pattern = format!("{prefix}%");
        let entries = stmt
            .query_map(rusqlite::params![command, pattern, limit], |row| {
                Ok(CompletionEntry {
                    command: row.get(0)?,
                    completion: row.get(1)?,
                    description: row.get(2)?,
                    source: row.get(3)?,
                })
            })?
            .filter_map(Result::ok)
            .collect();

        Ok(entries)
    }

    fn count(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("SqliteStore mutex poisoned: {e}"))?;
        let count: usize =
            conn.query_row("SELECT COUNT(*) FROM completions", [], |row| row.get(0))?;
        Ok(count)
    }
}

// ═══════════════════════════════════════════════════════════════════
// In-memory implementation (for testing)
// ═══════════════════════════════════════════════════════════════════

/// Simple `Vec`-backed store for unit tests.
pub struct MemStore {
    entries: Mutex<Vec<CompletionEntry>>,
}

impl MemStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for MemStore {
    fn insert(&self, entry: &CompletionEntry) -> Result<()> {
        let mut data = self
            .entries
            .lock()
            .map_err(|e| anyhow::anyhow!("MemStore mutex poisoned: {e}"))?;
        // Replace existing entry with same (command, completion, source)
        data.retain(|e| {
            !(e.command == entry.command
                && e.completion == entry.completion
                && e.source == entry.source)
        });
        data.push(entry.clone());
        Ok(())
    }

    fn query(
        &self,
        command: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<CompletionEntry>> {
        let data = self
            .entries
            .lock()
            .map_err(|e| anyhow::anyhow!("MemStore mutex poisoned: {e}"))?;
        let mut results: Vec<CompletionEntry> = data
            .iter()
            .filter(|e| e.command == command && e.completion.starts_with(prefix))
            .cloned()
            .collect();
        results.sort_by(|a, b| a.completion.cmp(&b.completion));
        results.truncate(limit);
        Ok(results)
    }

    fn count(&self) -> Result<usize> {
        let data = self
            .entries
            .lock()
            .map_err(|e| anyhow::anyhow!("MemStore mutex poisoned: {e}"))?;
        Ok(data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_store_empty() {
        let store = MemStore::new();
        assert_eq!(store.count().unwrap(), 0);
        let results = store.query("git", "", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn mem_store_insert_query() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            })
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let results = store.query("git", "co", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "commit");
    }

    #[test]
    fn mem_store_prefix_filter() {
        let store = MemStore::new();
        for name in ["commit", "cherry-pick", "clone", "checkout"] {
            store
                .insert(&CompletionEntry {
                    command: "git".into(),
                    completion: name.into(),
                    description: String::new(),
                    source: "fish".into(),
                })
                .unwrap();
        }
        let results = store.query("git", "ch", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.completion.starts_with("ch")));
    }

    #[test]
    fn mem_store_limit() {
        let store = MemStore::new();
        for i in 0..20 {
            store
                .insert(&CompletionEntry {
                    command: "test".into(),
                    completion: format!("opt-{i}"),
                    description: String::new(),
                    source: "mock".into(),
                })
                .unwrap();
        }
        let results = store.query("test", "", 5).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn mem_store_no_match() {
        let store = MemStore::new();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();
        // Wrong command
        let results = store.query("cargo", "co", 10).unwrap();
        assert!(results.is_empty());
        // Wrong prefix
        let results = store.query("git", "zzz", 10).unwrap();
        assert!(results.is_empty());
    }

    // ── SqliteStore tests ────────────────────────────────────────

    #[test]
    fn sqlite_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        assert_eq!(store.count().unwrap(), 0);

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            })
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);

        let results = store.query("git", "co", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "commit");
        assert_eq!(results[0].description, "Record changes");
    }

    #[test]
    fn sqlite_store_upsert_replaces_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "old description".into(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "new description".into(),
                source: "fish".into(),
            })
            .unwrap();

        assert_eq!(store.count().unwrap(), 1, "duplicate should be replaced, not added");
        let results = store.query("git", "co", 10).unwrap();
        assert_eq!(results[0].description, "new description");
    }

    #[test]
    fn sqlite_store_prefix_filter() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        for name in ["commit", "cherry-pick", "clone", "checkout"] {
            store
                .insert(&CompletionEntry {
                    command: "git".into(),
                    completion: name.into(),
                    description: String::new(),
                    source: "fish".into(),
                })
                .unwrap();
        }

        let results = store.query("git", "ch", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.completion.starts_with("ch")));
    }

    #[test]
    fn sqlite_store_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

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

        let results = store.query("test", "", 5).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn sqlite_store_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();

        assert!(store.query("cargo", "co", 10).unwrap().is_empty());
        assert!(store.query("git", "zzz", 10).unwrap().is_empty());
    }

    #[test]
    fn sqlite_store_empty_prefix_returns_all() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

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

        let results = store.query("git", "", 50).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn sqlite_store_multiple_sources_same_completion() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "from fish".into(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "from man".into(),
                source: "man".into(),
            })
            .unwrap();

        assert_eq!(
            store.count().unwrap(),
            2,
            "same completion from different sources should coexist"
        );
    }

    #[test]
    fn sqlite_store_open_or_create_succeeds() {
        let store = SqliteStore::open_or_create().unwrap();
        let _count = store.count().unwrap();
    }

    #[test]
    fn sqlite_store_validate_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();
        crate::testing::validate_store_roundtrip(&store);
    }

    #[test]
    fn sqlite_store_results_sorted_by_completion() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = SqliteStore::open_at(&db).unwrap();

        for name in ["zebra", "alpha", "middle"] {
            store
                .insert(&CompletionEntry {
                    command: "test".into(),
                    completion: name.into(),
                    description: String::new(),
                    source: "mock".into(),
                })
                .unwrap();
        }

        let results = store.query("test", "", 50).unwrap();
        let completions: Vec<&str> = results.iter().map(|r| r.completion.as_str()).collect();
        let mut sorted = completions.clone();
        sorted.sort_unstable();
        assert_eq!(completions, sorted, "results should be sorted by completion");
    }

    // ── CompletionEntry tests ────────────────────────────────────

    #[test]
    fn completion_entry_default() {
        let entry = CompletionEntry::default();
        assert!(entry.command.is_empty());
        assert!(entry.completion.is_empty());
        assert!(entry.description.is_empty());
        assert_eq!(entry.source, "custom");
    }

    #[test]
    fn completion_entry_serde_roundtrip() {
        let entry = CompletionEntry {
            command: "git".into(),
            completion: "commit".into(),
            description: "Record changes".into(),
            source: "fish".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: CompletionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn completion_entry_clone_eq() {
        let entry = CompletionEntry {
            command: "git".into(),
            completion: "commit".into(),
            description: "Record changes".into(),
            source: "fish".into(),
        };
        let cloned = entry.clone();
        assert_eq!(entry, cloned);
    }

    // ── MemStore upsert tests ────────────────────────────────────

    #[test]
    fn mem_store_upsert_replaces_duplicate() {
        let store = MemStore::new();

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "old".into(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "new".into(),
                source: "fish".into(),
            })
            .unwrap();

        assert_eq!(store.count().unwrap(), 1, "duplicate should be replaced");
        let results = store.query("git", "co", 10).unwrap();
        assert_eq!(results[0].description, "new");
    }

    #[test]
    fn mem_store_different_sources_coexist() {
        let store = MemStore::new();

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "fish".into(),
                source: "fish".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "man".into(),
                source: "man".into(),
            })
            .unwrap();

        assert_eq!(
            store.count().unwrap(),
            2,
            "same completion from different sources should coexist"
        );
    }

    #[test]
    fn mem_store_default_is_empty() {
        let store = MemStore::default();
        assert_eq!(store.count().unwrap(), 0);
    }

    // ── SqliteStore advanced tests ───────────────────────────────

    #[test]
    fn sqlite_store_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("persist.db");

        {
            let store = SqliteStore::open_at(&db).unwrap();
            store
                .insert(&CompletionEntry {
                    command: "git".into(),
                    completion: "commit".into(),
                    description: "Record changes".into(),
                    source: "fish".into(),
                })
                .unwrap();
            assert_eq!(store.count().unwrap(), 1);
        }

        let store = SqliteStore::open_at(&db).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        let results = store.query("git", "co", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "Record changes");
    }

    #[test]
    fn sqlite_store_special_characters_in_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("special.db");
        let store = SqliteStore::open_at(&db).unwrap();

        let entry = CompletionEntry {
            command: "git".into(),
            completion: "--format='%H %s'".into(),
            description: "it's a \"quoted\" value".into(),
            source: "fish".into(),
        };
        store.insert(&entry).unwrap();

        let results = store.query("git", "--format", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "--format='%H %s'");
        assert_eq!(results[0].description, "it's a \"quoted\" value");
    }

    #[test]
    fn sqlite_store_unicode_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("unicode.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "echo".into(),
                completion: "日本語".into(),
                description: "Japanese text — 漢字".into(),
                source: "custom".into(),
            })
            .unwrap();

        let results = store.query("echo", "日", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].completion, "日本語");
    }

    #[test]
    fn sqlite_store_bulk_insert_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("bulk.db");
        let store = SqliteStore::open_at(&db).unwrap();

        for i in 0..100 {
            store
                .insert(&CompletionEntry {
                    command: "test".into(),
                    completion: format!("option-{i:03}"),
                    description: format!("Description {i}"),
                    source: "mock".into(),
                })
                .unwrap();
        }

        assert_eq!(store.count().unwrap(), 100);

        let results = store.query("test", "option-05", 20).unwrap();
        assert_eq!(results.len(), 10, "option-050..option-059");

        let results = store.query("test", "option-09", 20).unwrap();
        assert_eq!(results.len(), 10, "option-090..option-099");
    }

    #[test]
    fn sqlite_store_empty_strings() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("empty_str.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "cmd".into(),
                completion: "opt".into(),
                description: String::new(),
                source: "fish".into(),
            })
            .unwrap();

        let results = store.query("cmd", "", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].description.is_empty());
    }

    #[test]
    fn sqlite_store_multiple_commands() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("multi_cmd.db");
        let store = SqliteStore::open_at(&db).unwrap();

        for cmd in ["git", "cargo", "docker", "kubectl"] {
            store
                .insert(&CompletionEntry {
                    command: cmd.into(),
                    completion: "help".into(),
                    description: format!("{cmd} help"),
                    source: "fish".into(),
                })
                .unwrap();
        }

        assert_eq!(store.count().unwrap(), 4);

        let git_results = store.query("git", "", 10).unwrap();
        assert_eq!(git_results.len(), 1);
        assert_eq!(git_results[0].description, "git help");

        let cargo_results = store.query("cargo", "", 10).unwrap();
        assert_eq!(cargo_results.len(), 1);
        assert_eq!(cargo_results[0].description, "cargo help");

        let all_results = store.query("nonexistent", "", 10).unwrap();
        assert!(all_results.is_empty());
    }

    #[test]
    fn sqlite_store_like_pattern_escaping() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("like.db");
        let store = SqliteStore::open_at(&db).unwrap();

        store
            .insert(&CompletionEntry {
                command: "test".into(),
                completion: "100%done".into(),
                description: String::new(),
                source: "mock".into(),
            })
            .unwrap();
        store
            .insert(&CompletionEntry {
                command: "test".into(),
                completion: "100_items".into(),
                description: String::new(),
                source: "mock".into(),
            })
            .unwrap();

        let results = store.query("test", "100", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn sqlite_store_concurrent_reads() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("concurrent.db");
        let store = std::sync::Arc::new(SqliteStore::open_at(&db).unwrap());

        store
            .insert(&CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            })
            .unwrap();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let s = std::sync::Arc::clone(&store);
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        let results = s.query("git", "co", 10).unwrap();
                        assert_eq!(results.len(), 1);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }
}
