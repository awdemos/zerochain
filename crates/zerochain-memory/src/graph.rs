use crate::graph_index::GraphIndex;
use crate::graph_store::ContributionStore;
use crate::record::ContributionRecord;
use crate::Result;

/// Workspace-level collective memory: canonical store plus derived index,
/// kept consistent on every publish (spec §4.2).
///
/// A `Graph` is a point-in-time view: it does not observe records published
/// through other handles after `open`. Keep at most one live handle per
/// directory per process, or re-`open` to see external writes. The canonical
/// files under `contributions/` are always the source of truth. `open`
/// creates the directory if missing — callers should verify the path.
#[derive(Debug)]
pub struct Graph {
    store: ContributionStore,
    index: GraphIndex,
}

impl Graph {
    pub async fn open(dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let store = ContributionStore::open(&dir).await?;
        let records = store.list().await?;
        Ok(Graph {
            store,
            index: GraphIndex::from_records(records),
        })
    }

    pub async fn publish(&mut self, record: ContributionRecord) -> Result<ContributionRecord> {
        let published = self.store.publish(record).await?;
        self.index.add(published.clone());
        Ok(published)
    }

    pub fn index(&self) -> &GraphIndex {
        &self.index
    }

    pub fn store(&self) -> &ContributionStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::ContributionType;
    use tempfile::TempDir;

    #[tokio::test]
    async fn publish_updates_index_and_reopen_rebuilds() {
        let tmp = TempDir::new().unwrap();
        let id = {
            let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
            let setup = graph
                .publish(ContributionRecord::new(
                    ContributionType::Setup,
                    "a",
                    "brief",
                ))
                .await
                .unwrap();
            assert_eq!(graph.index().len(), 1);
            setup.id
        };

        let graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        assert_eq!(graph.index().len(), 1);
        assert!(graph.index().get(&id).is_some());
    }

    #[tokio::test]
    async fn publish_failure_leaves_index_unchanged() {
        let tmp = TempDir::new().unwrap();
        let mut graph = Graph::open(tmp.path().join("graph")).await.unwrap();
        let mut orphan = ContributionRecord::new(ContributionType::Result, "a", "orphan");
        orphan.parents = vec!["c-missing00000000".to_string()];
        assert!(graph.publish(orphan).await.is_err());
        assert_eq!(graph.index().len(), 0);
    }
}
