use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use crate::{Broker, BrokerMessage, Result, Subscription};

/// In-memory broker for local development and testing.
///
/// Uses broadcast channels under the hood so multiple subscribers
/// can receive the same message.
#[derive(Clone)]
pub struct MemoryBroker {
    channels: Arc<Mutex<HashMap<String, broadcast::Sender<BrokerMessage>>>>,
}

impl MemoryBroker {
    /// Create a new in-memory broker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for MemoryBroker {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns true if `pattern` matches `subject` under NATS-style wildcard rules.
///
/// A `*` token in the pattern matches exactly one subject token (any string
/// without dots, including non-empty matches only). All other tokens must
/// match literally, and both subjects must have the same token count.
fn subject_matches(pattern: &str, subject: &str) -> bool {
    let mut pattern_tokens = pattern.split('.');
    let mut subject_tokens = subject.split('.');
    loop {
        match (pattern_tokens.next(), subject_tokens.next()) {
            (None, None) => return true,
            (Some("*"), Some(token)) if !token.is_empty() => continue,
            (Some(p), Some(s)) if p == s => continue,
            _ => return false,
        }
    }
}

#[async_trait::async_trait]
impl Broker for MemoryBroker {
    async fn publish(&self, subject: &str, msg: BrokerMessage) -> Result<()> {
        let channels = self.channels.lock().await;
        for (subscribed, tx) in channels.iter() {
            if subject_matches(subscribed, subject) {
                // Ignore send errors — just means no subscribers
                let _ = tx.send(msg.clone());
            }
        }
        Ok(())
    }

    async fn subscribe(&self, subject: &str) -> Result<Subscription> {
        let mut channels = self.channels.lock().await;
        let tx = channels
            .entry(subject.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        drop(channels);

        let mut rx = tx.subscribe();
        let (fwd_tx, fwd_rx) = mpsc::channel(256);
        let (cancel_tx, mut cancel_rx) = oneshot::channel();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => break,
                    result = rx.recv() => {
                        match result {
                            Ok(msg) => {
                                if fwd_tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                        }
                    }
                }
            }
        });

        Ok(Subscription::new(fwd_rx, cancel_tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrokerMessage;
    use zerochain_cas::Cid;

    fn dummy_cid() -> Cid {
        Cid::from_bytes(b"test")
    }

    fn dummy_msg() -> BrokerMessage {
        BrokerMessage::new("wf-1", "from", "to", dummy_cid())
    }

    #[tokio::test]
    async fn publish_without_subscriber_succeeds() {
        let broker = MemoryBroker::new();
        let result = broker.publish("test.subject", dummy_msg()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn subscribe_creates_receiver() {
        let broker = MemoryBroker::new();
        let rx = broker.subscribe("test.subject").await.unwrap();
        assert!(rx.is_empty());
    }

    #[tokio::test]
    async fn publish_delivers_to_subscriber() {
        let broker = MemoryBroker::new();
        let mut rx = broker.subscribe("test.subject").await.unwrap();

        let msg = dummy_msg();
        broker.publish("test.subject", msg.clone()).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.workflow_id, msg.workflow_id);
        assert_eq!(received.from_stage, msg.from_stage);
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_message() {
        let broker = MemoryBroker::new();
        let mut rx1 = broker.subscribe("test.subject").await.unwrap();
        let mut rx2 = broker.subscribe("test.subject").await.unwrap();

        let msg = dummy_msg();
        broker.publish("test.subject", msg).await.unwrap();

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert_eq!(r1.workflow_id, "wf-1");
        assert_eq!(r2.workflow_id, "wf-1");
    }

    #[tokio::test]
    async fn different_subjects_are_isolated() {
        let broker = MemoryBroker::new();
        let mut rx = broker.subscribe("subject.a").await.unwrap();

        broker.publish("subject.b", dummy_msg()).await.unwrap();

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn wildcard_subscriber_receives_matching_publish() {
        let broker = MemoryBroker::new();
        let mut rx = broker.subscribe("zerochain.*.*").await.unwrap();

        let msg = dummy_msg();
        broker
            .publish("zerochain.wf1.02_next", msg.clone())
            .await
            .unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.workflow_id, msg.workflow_id);
        assert_eq!(received.to_stage, msg.to_stage);
    }

    #[tokio::test]
    async fn wildcard_matches_exactly_one_token_per_star() {
        let broker = MemoryBroker::new();
        let mut rx = broker.subscribe("zerochain.*.*").await.unwrap();

        // Too few tokens: `*` must match exactly one token.
        broker.publish("zerochain.wf1", dummy_msg()).await.unwrap();
        // Too many tokens.
        broker
            .publish("zerochain.wf1.02_next.extra", dummy_msg())
            .await
            .unwrap();

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn wildcard_and_exact_subscribers_both_receive() {
        let broker = MemoryBroker::new();
        let mut wildcard_rx = broker.subscribe("zerochain.*.*").await.unwrap();
        let mut exact_rx = broker.subscribe("zerochain.wf1.02_next").await.unwrap();

        broker
            .publish("zerochain.wf1.02_next", dummy_msg())
            .await
            .unwrap();

        assert_eq!(wildcard_rx.recv().await.unwrap().workflow_id, "wf-1");
        assert_eq!(exact_rx.recv().await.unwrap().workflow_id, "wf-1");
    }
}
