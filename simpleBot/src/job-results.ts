import type { FunctionTool } from "./model.ts";
import type { TickSession } from "./tick.ts";
import { object } from "./tick.ts";

export const READ_JOB_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "read_job",
    description:
      "Read a bounded page of an existing terminal job result in this conversation. Use for truncated results, never to poll running jobs; use next.on_jobs to await completion.",
    parameters: {
      type: "object",
      properties: {
        job: { type: "string", minLength: 1, maxLength: 256 },
        offset: { type: "integer", minimum: 0 },
        limit: { type: "integer", minimum: 1, maximum: 8000 },
      },
      required: ["job"],
      additionalProperties: false,
    },
  },
};

export function readJob(session: TickSession | undefined, raw: string): string {
  const input: unknown = JSON.parse(raw);
  if (
    !object(input) ||
    typeof input.job !== "string" ||
    Object.keys(input).some((key) => !["job", "offset", "limit"].includes(key))
  )
    throw new Error("Invalid job page arguments");
  const offset = input.offset ?? 0;
  const limit = input.limit ?? 4000;
  if (
    typeof offset !== "number" ||
    !Number.isSafeInteger(offset) ||
    offset < 0 ||
    typeof limit !== "number" ||
    !Number.isSafeInteger(limit) ||
    limit < 1 ||
    limit > 8000
  )
    throw new Error("Invalid job page bounds");
  const job = session?.jobs[input.job];
  if (!job)
    return JSON.stringify({ error: "Unknown job in this conversation" });
  if (["queued", "running"].includes(job.status))
    return JSON.stringify({
      error: "Job is not terminal; use next.on_jobs instead of polling",
    });
  const result = job.result ?? "";
  return JSON.stringify({
    job: job.id,
    status: job.status,
    text: result.slice(offset, offset + limit),
    offset,
    nextOffset: offset + limit < result.length ? offset + limit : null,
    length: result.length,
  });
}
