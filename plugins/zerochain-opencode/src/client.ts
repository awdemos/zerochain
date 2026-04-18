import type { SimpleMessage, InitWorkflowRequest, RejectRequest, WorkflowStatus } from "./types";

const DEFAULT_BASE_URL = "http://localhost:8080";

export class ZerochainClient {
  private baseUrl: string;

  constructor(baseUrl?: string) {
    this.baseUrl = baseUrl ?? process.env.ZEROCHAIN_SERVER_URL ?? DEFAULT_BASE_URL;
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = {};
    let payload: string | undefined;

    if (body !== undefined) {
      headers["content-type"] = "application/json";
      payload = JSON.stringify(body);
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

    const contentType = response.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
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
}
