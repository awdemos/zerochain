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
    },
  };
};

export default ZerochainPlugin;
