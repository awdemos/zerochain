use async_nats::Client;
use futures::stream::StreamExt;
use tokio::sync::mpsc;

use crate::{Broker, BrokerError, BrokerMessage, Result};

/// NATS-backed message broker.
#[derive(Clone)]
pub struct NatsBroker {
    client: Client,
}

impl NatsBroker {
    /// Connect to a NATS server.
    pub async fn connect(url: &str) -> Result<Self> {
        let client = async_nats::connect(url)
            .await
            .map_err(|e| BrokerError::Connection(e.to_string()))?;
        Ok(Self { client })
    }

    /// Create from the `ZEROCHAIN_BROKER_URL` environment variable.
    pub async fn from_env() -> Result<Self> {
        let url = std::env::var("ZEROCHAIN_BROKER_URL")
            .unwrap_or_else(|_| "nats://localhost:4222".into());
        Self::connect(&url).await
    }
}

#[async_trait::async_trait]
impl Broker for NatsBroker {
    async fn publish(&self, subject: &str, msg: BrokerMessage) -> Result<()> {
        let payload = serde_json::to_vec(&msg)?;
        self.client
            .publish(subject.to_string(), payload.into())
            .await
            .map_err(|e| BrokerError::Publish(e.to_string()))?;
        self.client
            .flush()
            .await
            .map_err(|e| BrokerError::Publish(e.to_string()))?;
        Ok(())
    }

    async fn subscribe(&self, subject: &str) -> Result<mpsc::Receiver<BrokerMessage>> {
        let mut subscriber = self
            .client
            .subscribe(subject.to_string())
            .await
            .map_err(|e| BrokerError::Subscribe(e.to_string()))?;

        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            while let Some(msg) = subscriber.next().await {
                let payload = msg.payload;
                match serde_json::from_slice::<BrokerMessage>(&payload) {
                    Ok(broker_msg) => {
                        if tx.send(broker_msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to deserialize broker message");
                    }
                }
            }
        });

        Ok(rx)
    }
}
