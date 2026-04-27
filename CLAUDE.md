# bm-complete — Cached Shell Completion Daemon

> **★★★ CSE / Knowable Construction.** This repo operates under **Constructive Substrate Engineering** — canonical specification at [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md). The Compounding Directive (operational rules: solve once, load-bearing fixes only, idiom-first, models stay current, direction beats velocity) is in the org-level pleme-io/CLAUDE.md ★★★ section. Read both before non-trivial changes.


## Build & Test

```bash
cargo build          # compile
cargo test           # 50 unit + 4 integration tests
cargo check          # type-check only
```

## Architecture

Fast shell completion daemon. Indexes fish completions into SQLite, serves
queries over a Unix socket. Trait-based design for testability.

### Pipeline

```
Fish completion files → FishSource → CompletionEntry[]
                                   ↓
                          Store (SQLite or Mem) → cached in compiled.json
                                   ↓
Daemon (Unix socket) ← CompletionEngine → completions::complete()
                                           ├── DirectoryNav (cd/z/pushd)
                                           ├── PathCompletion (/, ~, ./)
                                           ├── FlagCompletion (-/--)
                                           └── CommandArg (store query)
```

### Module Map

| Module | Purpose |
|--------|---------|
| `src/store.rs` | `Store` trait + `SqliteStore` (SQLite) + `MemStore` (testing) |
| `src/config.rs` | `CompletionConfig` trait + `Config` (figment YAML/env) + `TestConfig` |
| `src/completions.rs` | Core logic: `complete()`, `index_sources()`, `index_sources_cached()`, context classification, `PathProvider` trait + `FsPathProvider` + `DirEntry` |
| `src/engine.rs` | `CompletionEngine` trait + `DefaultEngine` (`SqliteStore` + `Config` + `Arc<dyn PathProvider>`) |
| `src/daemon.rs` | Unix socket server, accepts `Arc<dyn CompletionEngine>` |
| `src/source.rs` | `CompletionSource` trait + `FishSource` + `MockSource` |
| `src/cache.rs` | `CacheStore`/`Fingerprinter` traits + `FsCache`/`FsFingerprinter` + `MemCache` |
| `src/testing.rs` | `CompletionEntryBuilder`, `validate_store_roundtrip()`, `classify_context_suite()` |
| `src/lib.rs` | Module declarations + re-exports |
| `src/main.rs` | CLI: daemon, complete, index, status subcommands |
| `tests/cli.rs` | Integration tests (assert_cmd + tempfile) |

### Key Traits

- **`Store`** — `insert()`, `query()`, `count()` — abstracts SQLite vs in-memory
- **`CompletionConfig`** — `max_results()`, `index_path()`, `fish_completion_dirs()`, `cache_dir()`
- **`CompletionEngine`** — `complete(buffer, position)` → `Vec<CompletionEntry>`
- **`CompletionSource`** — `name()`, `entries()` — pluggable completion sources
- **`CacheStore`/`Fingerprinter`** — mtime-based cache invalidation
- **`PathProvider`** — `list_dir()`, `exists()`, `is_dir()`, `home_dir()` — abstracts filesystem for path completions

### CLI

```
bm-complete daemon --socket /tmp/bm-complete.socket    # start daemon
bm-complete complete --buffer "git --co" --position 10  # query completions
bm-complete index --fish-dir /usr/share/fish/completions # index sources
bm-complete status --socket /tmp/bm-complete.socket     # check daemon
```

### Configuration

`~/.config/bm-complete/bm-complete.yaml` (figment, env prefix `BM_COMPLETE_`)

```yaml
cache_dir: ~/.cache/bm-complete
fish_completion_dirs:
  - /usr/share/fish/completions
  - /usr/local/share/fish/completions
max_results: 50
index_man_pages: true
index_help_flags: true
index_path: true
```

## Design Decisions

- **Trait boundaries everywhere** — `Store`, `CompletionConfig`, `CompletionEngine`, `CompletionSource`, `PathProvider` all have traits for testability; all traits require `Send + Sync`
- **`MemStore` for fast tests** — in-memory Vec-based store, no SQLite needed
- **Cache invalidation via fingerprinting** — XOR of mtimes detects when re-indexing is needed
- **`Mutex<SqliteStore>`** in engine — rusqlite Connection is not Send; low-concurrency, fast queries make Mutex fine
- **Single store open** — daemon opens SQLite once at startup, not per-request

## Consumers

- **blackmatter-shell** — zsh widget integration via Unix socket
