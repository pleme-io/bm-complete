//! Shared test utilities — builders, validators, fixture data.

use crate::completions::CompletionContext;
use crate::store::{CompletionEntry, Store};

/// Fluent builder for [`CompletionEntry`] — avoids boilerplate in tests.
pub struct CompletionEntryBuilder {
    command: String,
    completion: String,
    description: String,
    source: String,
}

impl CompletionEntryBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            command: String::new(),
            completion: String::new(),
            description: String::new(),
            source: "mock".into(),
        }
    }

    #[must_use]
    pub fn command(mut self, cmd: &str) -> Self {
        self.command = cmd.into();
        self
    }

    #[must_use]
    pub fn completion(mut self, comp: &str) -> Self {
        self.completion = comp.into();
        self
    }

    #[must_use]
    pub fn description(mut self, desc: &str) -> Self {
        self.description = desc.into();
        self
    }

    #[must_use]
    pub fn source(mut self, src: &str) -> Self {
        self.source = src.into();
        self
    }

    #[must_use]
    pub fn build(self) -> CompletionEntry {
        CompletionEntry {
            command: self.command,
            completion: self.completion,
            description: self.description,
            source: self.source,
        }
    }
}

impl Default for CompletionEntryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Insert an entry, query it back, and assert round-trip equality.
///
/// Works with any `Store` implementation.
pub fn validate_store_roundtrip(store: &dyn Store) {
    let entry = CompletionEntryBuilder::new()
        .command("git")
        .completion("commit")
        .description("Record changes")
        .source("fish")
        .build();
    store.insert(&entry).expect("insert should succeed");

    let results = store
        .query("git", "co", 10)
        .expect("query should succeed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], entry);
}

/// Exhaustive set of (command, prefix, expected context) tuples covering every
/// variant of [`CompletionContext`].
pub fn classify_context_suite() -> Vec<(&'static str, &'static str, CompletionContext)> {
    vec![
        // DirectoryNav
        ("cd", "", CompletionContext::DirectoryNav),
        ("pushd", "foo", CompletionContext::DirectoryNav),
        ("popd", "", CompletionContext::DirectoryNav),
        ("z", "pro", CompletionContext::DirectoryNav),
        ("zoxide", "", CompletionContext::DirectoryNav),
        ("j", "src", CompletionContext::DirectoryNav),
        ("autojump", "~", CompletionContext::DirectoryNav),
        // PathCompletion
        ("ls", "/etc", CompletionContext::PathCompletion),
        ("cat", "~/doc", CompletionContext::PathCompletion),
        ("vim", "./src", CompletionContext::PathCompletion),
        ("rm", "../old", CompletionContext::PathCompletion),
        // FlagCompletion
        ("git", "--verbose", CompletionContext::FlagCompletion),
        ("ls", "-l", CompletionContext::FlagCompletion),
        ("cargo", "-p", CompletionContext::FlagCompletion),
        // CommandArg
        ("git", "commit", CompletionContext::CommandArg),
        ("kubectl", "get", CompletionContext::CommandArg),
        ("cargo", "build", CompletionContext::CommandArg),
        ("ls", "", CompletionContext::CommandArg),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completions::classify_context;
    use crate::store::MemStore;

    #[test]
    fn builder_defaults() {
        let entry = CompletionEntryBuilder::new().build();
        assert!(entry.command.is_empty());
        assert!(entry.completion.is_empty());
        assert!(entry.description.is_empty());
        assert_eq!(entry.source, "mock");
    }

    #[test]
    fn builder_fluent_api() {
        let entry = CompletionEntryBuilder::new()
            .command("cargo")
            .completion("build")
            .description("Compile the package")
            .source("fish")
            .build();
        assert_eq!(entry.command, "cargo");
        assert_eq!(entry.completion, "build");
        assert_eq!(entry.description, "Compile the package");
        assert_eq!(entry.source, "fish");
    }

    #[test]
    fn roundtrip_with_mem_store() {
        let store = MemStore::new();
        validate_store_roundtrip(&store);
    }

    #[test]
    fn classify_context_suite_is_exhaustive() {
        let suite = classify_context_suite();
        // Every variant should appear at least once
        let variants: std::collections::HashSet<_> =
            suite.iter().map(|(_, _, ctx)| *ctx).collect();
        assert!(variants.contains(&CompletionContext::DirectoryNav));
        assert!(variants.contains(&CompletionContext::PathCompletion));
        assert!(variants.contains(&CompletionContext::FlagCompletion));
        assert!(variants.contains(&CompletionContext::CommandArg));

        // Each tuple should match
        for (cmd, prefix, expected) in &suite {
            assert_eq!(
                classify_context(cmd, prefix),
                *expected,
                "classify_context({cmd:?}, {prefix:?}) should be {expected:?}"
            );
        }
    }
}
