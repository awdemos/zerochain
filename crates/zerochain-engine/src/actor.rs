use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use zerochain_core::workflow::Workflow;

use crate::error::DaemonError;
use crate::state::{AppState, InitWorkflowParams};

pub enum ActorMessage {
    InitWorkflow {
        name: String,
        template: Option<String>,
        respond: oneshot::Sender<Result<Workflow, DaemonError>>,
    },
    RunStage {
        workflow_id: String,
        stage_raw: String,
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
    RunNext {
        workflow_id: String,
        respond: oneshot::Sender<Result<Option<String>, DaemonError>>,
    },
    GetWorkflow {
        workflow_id: String,
        respond: oneshot::Sender<Option<Workflow>>,
    },
    ListWorkflows {
        respond: oneshot::Sender<Vec<(String, String)>>,
    },
    SnapshotStage {
        workflow_id: String,
        stage_id: String,
        respond: oneshot::Sender<Result<PathBuf, DaemonError>>,
    },
    RestoreStage {
        workflow_id: String,
        stage_id: String,
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
    MarkStageComplete {
        workflow_id: String,
        stage_id: String,
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
    MarkStageError {
        workflow_id: String,
        stage_id: String,
        feedback: Option<String>,
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
    ReloadWorkflow {
        workflow_id: String,
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
    LoadWorkflows {
        respond: oneshot::Sender<Result<(), DaemonError>>,
    },
}

pub struct WorkflowActor {
    rx: mpsc::Receiver<ActorMessage>,
    state: AppState,
}

impl WorkflowActor {
    pub fn new(rx: mpsc::Receiver<ActorMessage>, state: AppState) -> Self {
        Self { rx, state }
    }

    pub async fn run(mut self) {
        while let Some(msg) = self.rx.recv().await {
            self.handle_message(msg).await;
        }
        tracing::debug!("WorkflowActor shutting down");
    }

    async fn handle_message(&mut self, msg: ActorMessage) {
        match msg {
            ActorMessage::InitWorkflow {
                name,
                template,
                respond,
            } => {
                let params = InitWorkflowParams {
                    name: &name,
                    path: None,
                    template: template.as_deref(),
                };
                let result = self.state.init_workflow(params).await;
                let _ = respond.send(result);
            }
            ActorMessage::RunStage {
                workflow_id,
                stage_raw,
                respond,
            } => {
                let result = self.state.run_stage(&workflow_id, &stage_raw).await;
                let _ = respond.send(result);
            }
            ActorMessage::RunNext {
                workflow_id,
                respond,
            } => {
                let result = self.state.run_next_stage(&workflow_id).await;
                let _ = respond.send(result);
            }
            ActorMessage::GetWorkflow {
                workflow_id,
                respond,
            } => {
                let result = self.state.get_workflow(&workflow_id).cloned();
                let _ = respond.send(result);
            }
            ActorMessage::ListWorkflows { respond } => {
                let result = self.state.list_workflows();
                let _ = respond.send(result);
            }
            ActorMessage::SnapshotStage {
                workflow_id,
                stage_id,
                respond,
            } => {
                let result = self.state.snapshot_stage(&workflow_id, &stage_id).await;
                let _ = respond.send(result);
            }
            ActorMessage::RestoreStage {
                workflow_id,
                stage_id,
                respond,
            } => {
                let result = self.state.restore_stage(&workflow_id, &stage_id).await;
                let _ = respond.send(result);
            }
            ActorMessage::MarkStageComplete {
                workflow_id,
                stage_id,
                respond,
            } => {
                let result = self
                    .state
                    .mark_stage_complete(&workflow_id, &stage_id, None)
                    .await;
                let _ = respond.send(result);
            }
            ActorMessage::MarkStageError {
                workflow_id,
                stage_id,
                feedback,
                respond,
            } => {
                let result = self
                    .state
                    .mark_stage_error(&workflow_id, &stage_id, feedback.as_deref())
                    .await;
                let _ = respond.send(result);
            }
            ActorMessage::ReloadWorkflow {
                workflow_id,
                respond,
            } => {
                let result = self.state.reload_workflow(&workflow_id).await;
                let _ = respond.send(result);
            }
            ActorMessage::LoadWorkflows { respond } => {
                let result = self.state.load_workflows().await;
                let _ = respond.send(result);
            }
        }
    }
}

#[derive(Clone)]
pub struct WorkflowHandle {
    tx: mpsc::Sender<ActorMessage>,
}

impl WorkflowHandle {
    pub fn spawn(state: AppState) -> Self {
        let (tx, rx) = mpsc::channel(64);
        let actor = WorkflowActor::new(rx, state);
        tokio::spawn(actor.run());
        Self { tx }
    }

    async fn call<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> ActorMessage,
    ) -> Result<T, DaemonError> {
        let (tx, rx) = oneshot::channel();
        let msg = build(tx);
        self.tx
            .send(msg)
            .await
            .map_err(|_| DaemonError::WorkflowLoadPartial("actor closed".into()))?;
        rx.await
            .map_err(|_| DaemonError::WorkflowLoadPartial("actor dropped".into()))
    }

    pub async fn init_workflow(
        &self,
        name: String,
        template: Option<String>,
    ) -> Result<Workflow, DaemonError> {
        self.call(|respond| ActorMessage::InitWorkflow {
            name,
            template,
            respond,
        })
        .await?
    }

    pub async fn run_stage(
        &self,
        workflow_id: String,
        stage_raw: String,
    ) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::RunStage {
            workflow_id,
            stage_raw,
            respond,
        })
        .await?
    }

    pub async fn run_next(&self, workflow_id: String) -> Result<Option<String>, DaemonError> {
        self.call(|respond| ActorMessage::RunNext {
            workflow_id,
            respond,
        })
        .await?
    }

    pub async fn get_workflow(&self, workflow_id: String) -> Option<Workflow> {
        self.call(|respond| ActorMessage::GetWorkflow {
            workflow_id,
            respond,
        })
        .await
        .ok()?
    }

    pub async fn list_workflows(&self) -> Vec<(String, String)> {
        self.call(|respond| ActorMessage::ListWorkflows { respond })
            .await
            .unwrap_or_default()
    }

    pub async fn snapshot_stage(
        &self,
        workflow_id: String,
        stage_id: String,
    ) -> Result<PathBuf, DaemonError> {
        self.call(|respond| ActorMessage::SnapshotStage {
            workflow_id,
            stage_id,
            respond,
        })
        .await?
    }

    pub async fn restore_stage(
        &self,
        workflow_id: String,
        stage_id: String,
    ) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::RestoreStage {
            workflow_id,
            stage_id,
            respond,
        })
        .await?
    }

    pub async fn mark_stage_complete(
        &self,
        workflow_id: String,
        stage_id: String,
    ) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::MarkStageComplete {
            workflow_id,
            stage_id,
            respond,
        })
        .await?
    }

    pub async fn mark_stage_error(
        &self,
        workflow_id: String,
        stage_id: String,
        feedback: Option<String>,
    ) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::MarkStageError {
            workflow_id,
            stage_id,
            feedback,
            respond,
        })
        .await?
    }

    pub async fn reload_workflow(&self, workflow_id: String) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::ReloadWorkflow {
            workflow_id,
            respond,
        })
        .await?
    }

    pub async fn load_workflows(&self) -> Result<(), DaemonError> {
        self.call(|respond| ActorMessage::LoadWorkflows { respond })
            .await?
    }
}
