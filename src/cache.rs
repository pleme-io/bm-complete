use std::path::PathBuf;
use std::{env, fs};

use crate::store::CompletionEntry;

/// Cached compiled completions format.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CompiledCache {
    pub fingerprint: u64,
    pub entries: Vec<CompletionEntry>,
}

/// Trait for cache storage — abstracts filesystem for testability.
pub trait CacheStore {
    fn load(&self) -> Option<CompiledCache>;
    fn save(&self, cache: &CompiledCache) -> anyhow::Result<()>;
}

/// Trait for fingerprinting — abstracts filesystem stat calls.
pub trait Fingerprinter {
    fn fingerprint(&self) -> u64;
}

// ═══════════════════════════════════════════════════════════════════
// Filesystem implementations
// ═══════════════════════════════════════════════════════════════════

/// Cache stored at `~/.cache/bm-complete/compiled.json`.
pub struct FsCache {
    pub path: PathBuf,
}

impl FsCache {
    #[must_use]
    pub fn default_path() -> PathBuf {
        env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env::var("HOME").unwrap_or_default()).join(".cache")
            })
            .join("bm-complete/compiled.json")
    }
}

impl CacheStore for FsCache {
    fn load(&self) -> Option<CompiledCache> {
        let content = fs::read(&self.path).ok()?;
        serde_json::from_slice(&content).ok()
    }

    fn save(&self, cache: &CompiledCache) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_vec(cache)?)?;
        Ok(())
    }
}

/// Fingerprint based on file mtimes in fish completion directories.
pub struct FsFingerprinter {
    pub dirs: Vec<PathBuf>,
}

impl Fingerprinter for FsFingerprinter {
    fn fingerprint(&self) -> u64 {
        let mut hash: u64 = 0;
        for dir in &self.dirs {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            hash ^= mtime_nanos(mtime);
                        }
                    }
                }
            }
        }
        hash
    }
}

fn mtime_nanos(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

// ═══════════════════════════════════════════════════════════════════
// In-memory implementations (for testing)
// ═══════════════════════════════════════════════════════════════════

/// In-memory cache for testing.
pub struct MemCache {
    pub data: std::cell::RefCell<Option<CompiledCache>>,
}

impl MemCache {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            data: std::cell::RefCell::new(None),
        }
    }
}

impl CacheStore for MemCache {
    fn load(&self) -> Option<CompiledCache> {
        self.data.borrow().as_ref().map(|c| CompiledCache {
            fingerprint: c.fingerprint,
            entries: c.entries.clone(),
        })
    }

    fn save(&self, cache: &CompiledCache) -> anyhow::Result<()> {
        *self.data.borrow_mut() = Some(CompiledCache {
            fingerprint: cache.fingerprint,
            entries: cache.entries.clone(),
        });
        Ok(())
    }
}

/// Fixed fingerprint for testing.
pub struct FixedFingerprinter(pub u64);

impl Fingerprinter for FixedFingerprinter {
    fn fingerprint(&self) -> u64 {
        self.0
    }
}

// ═══════════════════════════════════════════════════════════════════
// Resolver: cache-aware completion resolution
// ═══════════════════════════════════════════════════════════════════

/// Resolve completions with caching. Try cache first, fall back to
/// index function, auto-populate cache on miss.
pub fn resolve_cached(
    cache: &dyn CacheStore,
    fp: &dyn Fingerprinter,
    index_fn: impl FnOnce() -> anyhow::Result<Vec<CompletionEntry>>,
) -> anyhow::Result<Vec<CompletionEntry>> {
    let current_fp = fp.fingerprint();

    // Cache hit
    if let Some(cached) = cache.load() {
        if cached.fingerprint == current_fp {
            return Ok(cached.entries);
        }
    }

    // Cache miss — resolve and save
    let entries = index_fn()?;
    let _ = cache.save(&CompiledCache {
        fingerprint: current_fp,
        entries: entries.clone(),
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entries() -> Vec<CompletionEntry> {
        vec![CompletionEntry {
            command: "git".into(),
            completion: "commit".into(),
            description: "Record changes".into(),
            source: "fish".into(),
        }]
    }

    #[test]
    fn cache_miss_resolves_and_saves() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(42);
        let entries = resolve_cached(&cache, &fp, || Ok(test_entries())).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(cache.load().is_some());
        assert_eq!(cache.load().unwrap().fingerprint, 42);
    }

    #[test]
    fn cache_hit_skips_resolution() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(42);
        cache
            .save(&CompiledCache {
                fingerprint: 42,
                entries: test_entries(),
            })
            .unwrap();
        let entries = resolve_cached(&cache, &fp, || {
            panic!("should not be called on cache hit");
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn stale_cache_resolves_fresh() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(99);
        cache
            .save(&CompiledCache {
                fingerprint: 42,
                entries: vec![],
            })
            .unwrap();
        let entries = resolve_cached(&cache, &fp, || Ok(test_entries())).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(cache.load().unwrap().fingerprint, 99);
    }

    #[test]
    fn mem_cache_empty_returns_none() {
        let cache = MemCache::empty();
        assert!(cache.load().is_none());
    }

    #[test]
    fn fixed_fingerprinter() {
        let fp = FixedFingerprinter(12345);
        assert_eq!(fp.fingerprint(), 12345);
    }
}
