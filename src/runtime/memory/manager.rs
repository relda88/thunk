use std::sync::Arc;

use crate::runtime::index::EmbeddingProvider;
use crate::storage::memory::{MemoryFact, MemorySource, MemoryStore};

pub(crate) struct MemoryManager {
    pub(crate) store: MemoryStore,
    provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
}

impl MemoryManager {
    pub(crate) fn new(
        store: MemoryStore,
        provider: Option<Arc<dyn EmbeddingProvider + Send + Sync>>,
    ) -> Self {
        Self { store, provider }
    }

    /// Recall relevant facts for a query. Embeds the query and runs cosine recall;
    /// falls back to keyword search when no provider is configured.
    /// Always returns empty vec on error — never propagates.
    pub(crate) fn recall(&self, query: &str, scope: Option<&str>, limit: usize) -> Vec<MemoryFact> {
        if let Some(ref provider) = self.provider {
            match provider.embed(&[query.to_string()]) {
                Ok(embeddings) if !embeddings.is_empty() && !embeddings[0].is_empty() => {
                    match self.store.cosine_recall(&embeddings[0], scope, limit) {
                        Ok(facts) => return facts,
                        Err(_) => {}
                    }
                }
                _ => {}
            }
        }
        // Keyword fallback
        match self.store.list_facts(scope) {
            Ok(mut facts) => {
                let q = query.to_lowercase();
                facts.retain(|f| f.text.to_lowercase().contains(&q));
                facts.sort_by(|a, b| {
                    b.salience
                        .partial_cmp(&a.salience)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                facts.truncate(limit);
                facts
            }
            Err(_) => vec![],
        }
    }

    /// Top-N facts by salience for session-start anchor injection.
    /// Scope-filtered: global facts + project-scoped facts for current scope.
    pub(crate) fn anchor_facts(&self, scope: Option<&str>, limit: usize) -> Vec<MemoryFact> {
        match self.store.list_facts(scope) {
            Ok(mut facts) => {
                facts.sort_by(|a, b| {
                    b.salience
                        .partial_cmp(&a.salience)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                facts.truncate(limit);
                facts
            }
            Err(_) => vec![],
        }
    }

    /// Construct a MemoryFact proposal without persisting it.
    /// Caller is responsible for approval-gating and upsert.
    pub(crate) fn propose_fact(
        text: String,
        category: String,
        scope: Option<String>,
        source: MemorySource,
    ) -> MemoryFact {
        let now = now_str();
        MemoryFact {
            id: 0,
            text,
            category,
            scope,
            salience: 1.0,
            embedding: None,
            model_name: None,
            source,
            created_at: now.clone(),
            updated_at: now,
            last_recalled_at: None,
        }
    }
}

fn now_str() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn open_test_manager() -> (MemoryManager, tempfile::NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let store = MemoryStore::open(tmp.path()).unwrap();
        (MemoryManager::new(store, None), tmp)
    }

    #[test]
    fn recall_keyword_fallback_no_provider() {
        let (mgr, _tmp) = open_test_manager();
        mgr.store
            .upsert_fact(
                "I prefer Rust for systems programming",
                "preference",
                None,
                1.0,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();
        mgr.store
            .upsert_fact(
                "I drink coffee in the morning",
                "habit",
                None,
                1.0,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();

        let results = mgr.recall("rust", None, 10);
        assert_eq!(results.len(), 1);
        assert!(results[0].text.to_lowercase().contains("rust"));
    }

    #[test]
    fn anchor_facts_sorted_by_salience() {
        let (mgr, _tmp) = open_test_manager();
        mgr.store
            .upsert_fact(
                "fact low",
                "test",
                None,
                0.5,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();
        mgr.store
            .upsert_fact(
                "fact high",
                "test",
                None,
                1.0,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();
        mgr.store
            .upsert_fact(
                "fact mid",
                "test",
                None,
                0.8,
                None,
                None,
                MemorySource::User,
            )
            .unwrap();

        let results = mgr.anchor_facts(None, 2);
        assert_eq!(results.len(), 2);
        assert!(results[0].salience >= results[1].salience);
        assert_eq!(results[0].text, "fact high");
        assert_eq!(results[1].text, "fact mid");
    }

    #[test]
    fn propose_fact_does_not_persist() {
        let (mgr, _tmp) = open_test_manager();
        let _fact = MemoryManager::propose_fact(
            "ephemeral fact".to_string(),
            "test".to_string(),
            None,
            MemorySource::User,
        );
        let stored = mgr.store.list_facts(None).unwrap();
        assert!(
            stored.is_empty(),
            "propose_fact must not write to the store"
        );
    }
}
