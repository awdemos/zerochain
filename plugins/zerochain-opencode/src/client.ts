import type { SimpleMessage, InitWorkflowRequest, RejectRequest, WorkflowStatus, ArtifactResponse, BrokerMessage } from "./types";

const DEFAULT_BASE_URL = "http://localhost:8080";

export class ZerochainClient {
  private baseUrl: string;
  private apiKey: string | undefined;

  constructor(baseUrl?: string) {
    this.baseUrl = baseUrl ?? process.env.ZEROCHAIN_SERVER_URL ?? DEFAULT_BASE_URL;
    this.apiKey = process.env.ZEROCHAIN_API_KEY;
  }

  private async request<T>(method: string, path: string, body?: unknown, contentType?: string): Promise<T> {
    const headers: Record<string, string> = {};
    let payload: string | ArrayBuffer | undefined;

    if (this.apiKey) {
      headers["authorization"] = `Bearer ${this.apiKey}`;
    }

    if (body !== undefined) {
      if (contentType === "application/octet-stream" && body instanceof ArrayBuffer) {
        payload = body;
        headers["content-type"] = "application/octet-stream";
      } else {
        headers["content-type"] = "application/json";
        payload = JSON.stringify(body);
      }
    }

    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers,
      body: payload,
    });

    if (!response.ok) {
      const text = await response.text().catch(() => "unknown error");
      throw new Error(`zerochain ${response.status}: ${text}`);
    }

    const respContentType = response.headers.get("content-type") ?? "";
    if (respContentType.includes("application/json")) {
      return response.json() as Promise<T>;
    }
    return response.text() as Promise<T>;
  }

  async health(): Promise<string> {
    return this.request<string>("GET", "/v1/health");
  }

  async listWorkflows(): Promise<SimpleMessage[]> {
    return this.request<SimpleMessage[]>("GET", "/v1/workflows");
  }

  async initWorkflow(name: string, template?: string): Promise<SimpleMessage> {
    const body: InitWorkflowRequest = { name };
    if (template) body.template = template;
    return this.request<SimpleMessage>("POST", "/v1/workflows", body);
  }

  async getWorkflow(id: string): Promise<WorkflowStatus> {
    return this.request<WorkflowStatus>("GET", `/v1/workflows/${id}`);
  }

  async runNext(id: string): Promise<SimpleMessage> {
    return this.request<SimpleMessage>("POST", `/v1/workflows/${id}/run`);
  }

  async runStage(id: string, stage: string): Promise<SimpleMessage> {
    return this.request<SimpleMessage>("POST", `/v1/workflows/${id}/run/${stage}`);
  }

  async approve(id: string, stage: string): Promise<SimpleMessage> {
    return this.request<SimpleMessage>("POST", `/v1/workflows/${id}/approve/${stage}`);
  }

  async reject(id: string, stage: string, feedback?: string): Promise<SimpleMessage> {
    const body: RejectRequest = {};
    if (feedback) body.feedback = feedback;
    return this.request<SimpleMessage>("POST", `/v1/workflows/${id}/reject/${stage}`, body);
  }

  async readOutput(id: string, stage: string): Promise<string> {
    return this.request<string>("GET", `/v1/workflows/${id}/output/${stage}`);
  }

  async readReasoning(id: string, stage: string): Promise<string> {
    return this.request<string>("GET", `/v1/workflows/${id}/reasoning/${stage}`);
  }

  async uploadArtifact(data: ArrayBuffer): Promise<ArtifactResponse> {
    return this.request<ArtifactResponse>("POST", "/v1/artifacts", data, "application/octet-stream");
  }

  async listArtifacts(): Promise<string[]> {
    return this.request<string[]>("GET", "/v1/artifacts");
  }

  async downloadArtifact(cid: string): Promise<string> {
    return this.request<string>("GET", `/v1/artifacts/${cid}`);
  }

  async sendPrompt(workflowId: string, fromStage: string, toStage: string, content: string): Promise<SimpleMessage> {
    return this.request<SimpleMessage>(
      "POST",
      `/v1/workflows/${workflowId}/stages/${fromStage}/prompt`,
      { to_stage: toStage, content },
    );
  }

  async pollPrompts(workflowId: string, stage: string): Promise<BrokerMessage | SimpleMessage> {
    return this.request<BrokerMessage | SimpleMessage>(
      "GET",
      `/v1/workflows/${workflowId}/stages/${stage}/poll`,
    );
  }
}
