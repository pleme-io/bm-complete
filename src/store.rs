use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionEntry {
    pub command: String,
    pub completion: String,
    pub description: String,
    pub source: String, // "fish", "man", "help", "path", "custom"
}

pub struct CompletionStore {
    conn: Connection,
}

impl CompletionStore {
    pub fn open_or_create() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("bm-complete");
        std::fs::create_dir_all(&cache_dir)?;
        let db_path = cache_dir.join("completions.db");
        let conn = Connection::open(&db_path)
            .context("failed to open completion database")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS completions (
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

        Ok(Self { conn })
    }

    pub fn insert(&self, entry: &CompletionEntry) -> Result<()> {
        self.conn.execute(
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

    pub fn query(&self, command: &str, prefix: &str, limit: usize) -> Result<Vec<CompletionEntry>> {
        let mut stmt = self.conn.prepare(
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
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    pub fn count(&self) -> Result<usize> {
        let count: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM completions", [], |row| row.get(0))?;
        Ok(count)
    }
}
