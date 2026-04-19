import { Plugin, tool } from "@opencode-ai/plugin";
import { ZerochainClient } from "./client";

const getServerUrl = (): string =>
  process.env.ZEROCHAIN_SERVER_URL ?? "http://localhost:8080";

export const ZerochainPlugin: Plugin = async (_ctx) => {
  return {
    tool: {
      zerochain_init: tool({
        description: "Initialize a new zerochain workflow. Optionally specify a built-in template (code-review, research, implement) or comma-separated stage names.",
        args: {
          name: tool.schema.string().describe("Workflow name"),
          template: tool.schema.string().optional().describe("Template name (code-review, research, implement) or comma-separated stages"),
        },
        async execute({ name, template }) {
          const client = new ZerochainClient(getServerUrl());
          const result = await client.initWorkflow(name as string, template as string | undefined);
          return `Workflow initialized: ${result.message}`;
        },
      }),

      zerochain_run: tool({
        description: "Execute the next pending stage in a zerochain workflow, or a specific stage.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().optional().describe("Specific stage to run (e.g. 02_design). Omit to run next pending."),
        },
        async execute({ workflow_id, stage }) {
          const client = new ZerochainClient(getServerUrl());
          const wid = workflow_id as string;
          const stg = stage as string | undefined;
          const result = stg
            ? await client.runStage(wid, stg)
            : await client.runNext(wid);
          return result.message;
        },
      }),

      zerochain_status: tool({
        description: "Check the status of a zerochain workflow, listing all stages and their states.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
        },
        async execute({ workflow_id }) {
          const client = new ZerochainClient(getServerUrl());
          const wf = await client.getWorkflow(workflow_id as string);
          const lines = [`Workflow: ${wf.id} (${wf.status})`];
          for (const s of wf.stages) {
            const marker = s.complete ? "done" : s.error ? "error" : s.human_gate ? "gate" : "pending";
            lines.push(`  ${s.id} [${marker}]`);
          }
          return lines.join("\n");
        },
      }),

      zerochain_list: tool({
        description: "List all zerochain workflows and their statuses.",
        args: {},
        async execute() {
          const client = new ZerochainClient(getServerUrl());
          const list = await client.listWorkflows();
          if (list.length === 0) return "No workflows found.";
          return list.map((w) => w.message).join("\n");
        },
      }),

      zerochain_approve: tool({
        description: "Approve a zerochain stage that is waiting at a human gate.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().describe("Stage ID (e.g. 03_review)"),
        },
        async execute({ workflow_id, stage }) {
          const client = new ZerochainClient(getServerUrl());
          const result = await client.approve(workflow_id as string, stage as string);
          return result.message;
        },
      }),

      zerochain_reject: tool({
        description: "Reject a zerochain stage and mark it as error, with optional feedback.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().describe("Stage ID (e.g. 03_review)"),
          feedback: tool.schema.string().optional().describe("Feedback for why the stage was rejected"),
        },
        async execute({ workflow_id, stage, feedback }) {
          const client = new ZerochainClient(getServerUrl());
          const result = await client.reject(workflow_id as string, stage as string, feedback as string | undefined);
          return result.message;
        },
      }),

      zerochain_output: tool({
        description: "Read the output (result.md) of a completed zerochain stage.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().describe("Stage ID (e.g. 00_spec)"),
        },
        async execute({ workflow_id, stage }) {
          const client = new ZerochainClient(getServerUrl());
          return await client.readOutput(workflow_id as string, stage as string);
        },
      }),

      zerochain_reasoning: tool({
        description: "Read the reasoning/chain-of-thought output of a zerochain stage (if captured).",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().describe("Stage ID"),
        },
        async execute({ workflow_id, stage }) {
          const client = new ZerochainClient(getServerUrl());
          return await client.readReasoning(workflow_id as string, stage as string);
        },
      }),

      zerochain_upload_artifact: tool({
        description: "Upload a binary artifact to zerochain's content-addressed store. Returns a CID (content hash) for retrieval.",
        args: {
          data: tool.schema.string().describe("Base64-encoded artifact data"),
        },
        async execute({ data }) {
          const client = new ZerochainClient(getServerUrl());
          const binary = Uint8Array.from(atob(data as string), (c) => c.charCodeAt(0));
          const result = await client.uploadArtifact(binary.buffer as ArrayBuffer);
          return `Artifact uploaded: ${result.cid}`;
        },
      }),

      zerochain_download_artifact: tool({
        description: "Download an artifact from zerochain's content-addressed store by its CID.",
        args: {
          cid: tool.schema.string().describe("Content ID (CID) of the artifact"),
        },
        async execute({ cid }) {
          const client = new ZerochainClient(getServerUrl());
          return await client.downloadArtifact(cid as string);
        },
      }),

      zerochain_list_artifacts: tool({
        description: "List all artifact CIDs in zerochain's content-addressed store.",
        args: {},
        async execute() {
          const client = new ZerochainClient(getServerUrl());
          const cids = await client.listArtifacts();
          if (cids.length === 0) return "No artifacts stored.";
          return cids.join("\n");
        },
      }),

      zerochain_send_prompt: tool({
        description: "Send a cross-pod prompt from one stage to another. Used for multi-agent communication within a workflow.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          from_stage: tool.schema.string().describe("Source stage ID"),
          to_stage: tool.schema.string().describe("Target stage ID"),
          content: tool.schema.string().describe("Prompt content to send"),
        },
        async execute({ workflow_id, from_stage, to_stage, content }) {
          const client = new ZerochainClient(getServerUrl());
          const result = await client.sendPrompt(
            workflow_id as string,
            from_stage as string,
            to_stage as string,
            content as string,
          );
          return result.message;
        },
      }),

      zerochain_poll_prompts: tool({
        description: "Poll for pending prompts/messages addressed to a stage. Returns the latest message or timeout.",
        args: {
          workflow_id: tool.schema.string().describe("Workflow ID"),
          stage: tool.schema.string().describe("Stage ID to poll"),
        },
        async execute({ workflow_id, stage }) {
          const client = new ZerochainClient(getServerUrl());
          const result = await client.pollPrompts(workflow_id as string, stage as string);
          if ("message" in result) return (result as { message: string }).message;
          const msg = result as import("./types").BrokerMessage;
          return `From: ${msg.from_stage}, CID: ${msg.content_cid}, Timestamp: ${msg.timestamp}`;
        },
      }),
    },
  };
};

export default ZerochainPlugin;
