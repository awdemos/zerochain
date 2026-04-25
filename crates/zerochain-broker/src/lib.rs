//! Message broker abstraction for cross-pod agent communication.

use serde::{Deserialize, Serialize};
use zerochain_cas::Cid;

pub mod memory;
#[cfg(feature = "nats")]
pub mod nats;

/// A message exchanged between agents via the broker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BrokerMessage {
    /// Workflow this message belongs to.
    pub workflow_id: String,
    /// Stage that sent the message.
    pub from_stage: String,
    /// Stage that should receive the message.
    pub to_stage: String,
    /// CID of the actual prompt content in the CAS store.
    pub prompt_cid: Cid,
    /// Optional metadata (timestamps, headers, etc.).
    pub metadata: serde_json::Value,
}

impl BrokerMessage {
    /// Create a new broker message.
    pub fn new(
        workflow_id: impl Into<String>,
        from_stage: impl Into<String>,
        to_stage: impl Into<String>,
        prompt_cid: Cid,
    ) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            from_stage: from_stage.into(),
            to_stage: to_stage.into(),
            prompt_cid,
            metadata: serde_json::Value::Null,
        }
    }

    /// Attach metadata to the message.
    #[must_use] pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerochain_cas::Cid;

    fn dummy_cid() -> Cid {
        Cid::from_bytes(b"test")
    }

    #[test]
    fn broker_message_new() {
        let msg = BrokerMessage::new("wf-1", "stage-a", "stage-b", dummy_cid());
        assert_eq!(msg.workflow_id, "wf-1");
        assert_eq!(msg.from_stage, "stage-a");
        assert_eq!(msg.to_stage, "stage-b");
        assert!(msg.metadata.is_null());
    }

    #[test]
    fn broker_message_with_metadata() {
        let msg = BrokerMessage::new("wf-1", "a", "b", dummy_cid())
            .with_metadata(serde_json::json!({"priority": 1}));
        assert_eq!(msg.metadata["priority"], 1);
    }
}

/// Errors that can occur when interacting with the broker.
#[derive(thiserror::Error, Debug)]
pub enum BrokerError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("publish failed: {0}")]
    Publish(String),
    #[error("subscribe failed: {0}")]
    Subscribe(String),
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("CAS error: {0}")]
    Cas(String),
}

pub type Result<T> = std::result::Result<T, BrokerError>;

/// Abstraction over message broker backends.
#[async_trait::async_trait]
pub trait Broker: Send + Sync {
    /// Publish a message to the given subject.
    ///
    /// Subjects follow NATS conventions, e.g. `zerochain.{workflow_id}.{stage}`.
    async fn publish(&self, subject: &str, msg: BrokerMessage) -> Result<()>;

    /// Subscribe to messages on the given subject.
    ///
    /// Returns a channel that yields messages as they arrive.
    async fn subscribe(&self, subject: &str) -> Result<tokio::sync::mpsc::Receiver<BrokerMessage>>;
}
