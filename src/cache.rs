//! Cache layer — delegates to hayai's generic cache infrastructure.
//!
//! bm-complete's cache stores `Vec<CompletionEntry>` with mtime-based
//! fingerprinting. All traits, impls, and resolution logic come from hayai.

use std::path::PathBuf;

use crate::store::CompletionEntry;

// Re-export hayai cache types — consumers use these directly.
pub use hayai::cache::{
    CacheStore, FixedFingerprinter, FsCache, FsFingerprinter, Fingerprinter, MemCache,
    resolve_cached,
};

/// Create the default filesystem cache for bm-complete.
#[must_use]
pub fn default_cache() -> FsCache {
    FsCache::for_app("bm-complete")
}

/// Create a fingerprinter for fish completion directories.
#[must_use]
pub fn completion_fingerprinter(dirs: Vec<PathBuf>) -> FsFingerprinter {
    FsFingerprinter::from_dirs(dirs)
}

/// Cache-aware resolution wrapper typed to `Vec<CompletionEntry>`.
///
/// Thin wrapper around `hayai::cache::resolve_cached` — exists so callers
/// don't need to spell out the generic parameter.
pub fn resolve_completions(
    cache: &dyn CacheStore<Vec<CompletionEntry>>,
    fp: &dyn Fingerprinter,
    index_fn: impl FnOnce() -> anyhow::Result<Vec<CompletionEntry>>,
) -> anyhow::Result<Vec<CompletionEntry>> {
    resolve_cached(cache, fp, index_fn)
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
        let entries = resolve_completions(&cache, &fp, || Ok(test_entries())).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(cache.load().is_some());
        assert_eq!(cache.load().unwrap().0, 42);
    }

    #[test]
    fn cache_hit_skips_resolution() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(42);
        cache.save(42, &test_entries()).unwrap();
        let entries = resolve_completions(&cache, &fp, || {
            panic!("should not be called on cache hit");
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn stale_cache_resolves_fresh() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(99);
        cache.save(42, &vec![]).unwrap();
        let entries = resolve_completions(&cache, &fp, || Ok(test_entries())).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(cache.load().unwrap().0, 99);
    }

    #[test]
    fn mem_cache_empty_returns_none() {
        let cache: MemCache<Vec<CompletionEntry>> = MemCache::empty();
        assert!(cache.load().is_none());
    }

    #[test]
    fn fixed_fingerprinter() {
        let fp = FixedFingerprinter(12345);
        assert_eq!(fp.fingerprint(), 12345);
    }

    #[test]
    fn default_cache_path() {
        let cache = default_cache();
        assert!(cache.path.to_str().unwrap().contains("bm-complete"));
        assert!(cache.path.to_str().unwrap().contains("compiled.json"));
    }

    #[test]
    fn fs_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FsCache {
            path: dir.path().join("compiled.json"),
        };

        assert!(CacheStore::<Vec<CompletionEntry>>::load(&cache).is_none());

        let original = test_entries();
        cache.save(42, &original).unwrap();

        let (fp, loaded) = CacheStore::<Vec<CompletionEntry>>::load(&cache)
            .expect("cache should exist after save");
        assert_eq!(fp, 42);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "git");
    }

    #[test]
    fn fs_fingerprinter_changes_on_file_modify() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.fish");
        std::fs::write(&file_path, "original content").unwrap();

        let fp = completion_fingerprinter(vec![dir.path().to_path_buf()]);
        let fp1 = fp.fingerprint();

        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "modified content").unwrap();

        let fp2 = fp.fingerprint();
        assert_ne!(fp1, fp2, "fingerprint should change when file is modified");
    }

    #[test]
    fn resolve_completions_error_propagation() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(1);
        let result = resolve_completions(&cache, &fp, || {
            Err(anyhow::anyhow!("indexing failed"))
        });
        assert!(result.is_err(), "error from index_fn should propagate");
        assert!(
            result.unwrap_err().to_string().contains("indexing failed"),
            "error message should be preserved"
        );
    }

    #[test]
    fn cache_hit_preserves_data_fidelity() {
        let cache = MemCache::empty();
        let fp = FixedFingerprinter(42);
        let entries = vec![
            CompletionEntry {
                command: "git".into(),
                completion: "commit".into(),
                description: "Record changes".into(),
                source: "fish".into(),
            },
            CompletionEntry {
                command: "git".into(),
                completion: "push".into(),
                description: "Update remote".into(),
                source: "fish".into(),
            },
        ];
        cache.save(42, &entries).unwrap();
        let loaded = resolve_completions(&cache, &fp, || panic!("should not call")).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].completion, "commit");
        assert_eq!(loaded[1].completion, "push");
    }

    #[test]
    fn fs_cache_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FsCache {
            path: dir.path().join("compiled.json"),
        };

        let entries1 = vec![CompletionEntry {
            command: "v1".into(),
            completion: "old".into(),
            description: String::new(),
            source: "test".into(),
        }];
        cache.save(1, &entries1).unwrap();

        let entries2 = vec![CompletionEntry {
            command: "v2".into(),
            completion: "new".into(),
            description: String::new(),
            source: "test".into(),
        }];
        cache.save(2, &entries2).unwrap();

        let (fp, loaded) = CacheStore::<Vec<CompletionEntry>>::load(&cache).unwrap();
        assert_eq!(fp, 2);
        assert_eq!(loaded[0].completion, "new");
    }

    #[test]
    fn fs_fingerprinter_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let fp = completion_fingerprinter(vec![dir.path().to_path_buf()]);
        let fp1 = fp.fingerprint();
        let fp2 = fp.fingerprint();
        assert_eq!(fp1, fp2, "fingerprint of empty dir should be deterministic");
    }

    #[test]
    fn fs_fingerprinter_stable_without_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("stable.fish"), "content").unwrap();

        let fp = completion_fingerprinter(vec![dir.path().to_path_buf()]);
        let fp1 = fp.fingerprint();
        let fp2 = fp.fingerprint();
        assert_eq!(fp1, fp2, "fingerprint should be stable without file changes");
    }

    #[test]
    fn fs_fingerprinter_nonexistent_dir() {
        let fp = completion_fingerprinter(vec![std::path::PathBuf::from("/nonexistent/dir")]);
        let fp1 = fp.fingerprint();
        let fp2 = fp.fingerprint();
        assert_eq!(
            fp1, fp2,
            "fingerprint of nonexistent dir should be deterministic"
        );
    }

    #[test]
    fn mem_cache_save_and_load() {
        let cache: MemCache<Vec<CompletionEntry>> = MemCache::empty();
        let entries = test_entries();
        cache.save(99, &entries).unwrap();
        let (fp, loaded) = cache.load().unwrap();
        assert_eq!(fp, 99);
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn mem_cache_overwrite() {
        let cache: MemCache<Vec<CompletionEntry>> = MemCache::empty();
        cache.save(1, &vec![]).unwrap();
        cache.save(2, &test_entries()).unwrap();
        let (fp, loaded) = cache.load().unwrap();
        assert_eq!(fp, 2);
        assert_eq!(loaded.len(), 1);
    }
}
