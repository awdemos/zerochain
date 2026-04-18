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
