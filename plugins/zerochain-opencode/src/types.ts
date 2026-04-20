export interface WorkflowStatus {
  id: string;
  status: string;
  stages: StageStatus[];
}

export interface StageStatus {
  id: string;
  complete: boolean;
  error: boolean;
  human_gate: boolean;
}

export interface SimpleMessage {
  message: string;
}

export interface InitWorkflowRequest {
  name: string;
  template?: string;
}

export interface RejectRequest {
  feedback?: string;
}

export interface ArtifactResponse {
  cid: string;
}

export interface PromptRequest {
  to_stage: string;
  content: string;
}

export interface BrokerMessage {
  workflow_id: string;
  from_stage: string;
  to_stage: string;
  content_cid: string;
  timestamp: string;
  metadata: Record<string, string>;
}
