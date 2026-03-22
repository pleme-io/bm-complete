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
    fn insert(&self, entry: &CompletionEntry) -> Result<()>;
    /// Query completions for `command` whose completion text starts with `prefix`.
    fn query(&self, command: &str, prefix: &str, limit: usize) -> Result<Vec<CompletionEntry>>;
    /// Total number of stored entries.
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
    pub fn open_or_create() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("bm-complete");
        std::fs::create_dir_all(&cache_dir)?;
        let db_path = cache_dir.join("completions.db");
        Self::open_at(&db_path)
    }

    /// Open (or create) a database at an explicit path — useful for tests.
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

// ═══════════════════════════════════════════════════════════════════
// Backward-compat alias
// ═══════════════════════════════════════════════════════════════════

/// Legacy alias — prefer [`SqliteStore`] in new code.
pub type CompletionStore = SqliteStore;

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
}
