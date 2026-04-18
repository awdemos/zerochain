use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};

use crate::{Broker, BrokerMessage, Result};

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

#[async_trait::async_trait]
impl Broker for MemoryBroker {
    async fn publish(&self, subject: &str, msg: BrokerMessage) -> Result<()> {
        let channels = self.channels.lock().await;
        if let Some(tx) = channels.get(subject) {
            // Ignore send errors — just means no subscribers
            let _ = tx.send(msg);
        }
        Ok(())
    }

    async fn subscribe(&self, subject: &str) -> Result<mpsc::Receiver<BrokerMessage>> {
        let mut channels = self.channels.lock().await;
        let tx = channels
            .entry(subject.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        drop(channels);

        let mut rx = tx.subscribe();
        let (fwd_tx, fwd_rx) = mpsc::channel(256);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if fwd_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });

        Ok(fwd_rx)
    }
}
