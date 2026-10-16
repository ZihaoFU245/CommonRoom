import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "../src/agent.ts";
import { BotApi } from "../src/api.ts";
import { ConfigStore } from "../src/config.ts";
import { Store } from "../src/store.ts";
import type { CompletionRequest, ChatModel } from "../src/model.ts";
import { ModelClient } from "../src/model.ts";
import { tickDecision, type TickDecision } from "../src/tick.ts";
import { MEMORY_TOOL } from "../src/memory.ts";
import { readJob } from "../src/job-results.ts";
import { MockServer } from "./mock-server.ts";

const wait: TickDecision = {
  messages: [],
  actions: [],
  next: { kind: "wait" },
};

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 4000;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("Event did not arrive");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

async function fixture(
  tick: (request: CompletionRequest, call: number) => Promise<TickDecision>,
  withShell = false,
) {
  const server = new MockServer();
  const url = await server.listen();
  const dir = mkdtempSync(join(tmpdir(), "simplebot-loop-"));
  const config = ConfigStore.load(join(dir, "config.json"));
  config.set("chat.historyWindow", 0);
  let store = await Store.open(join(dir, "agent.kv"));
  const requests: CompletionRequest[] = [];
  const model: ChatModel = {
    describe: "tick-model",
    complete: () => {
      throw new Error("Hidden completion loop used");
    },
    tick: (request) => {
      requests.push(request);
      return tick(request, requests.length);
    },
  };
  const api = new BotApi({ baseUrl: url, token: server.token });
  const logs: string[] = [];
  const create = () =>
    new Agent({
      api,
      store,
      config,
      model,
      workspace: join(dir, "workspace"),
      withShell,
      log: (_level, text) => logs.push(text),
      presence: () => [],
      socketReady: () => true,
      setActivity: () => {},
    });
  let agent = create();
  await agent.handleSnapshot(server.snapshot());
  return {
    server,
    config,
    api,
    requests,
    logs,
    dir,
    get agent() {
      return agent;
    },
    get store() {
      return store;
    },
    async restart(nextShell = withShell) {
      await agent.stop();
      withShell = nextShell;
      await store.close();
      store = await Store.open(join(dir, "agent.kv"));
      agent = create();
      await agent.handleSnapshot(server.snapshot());
    },
    async close() {
      await agent.stop();
      await store.close();
      await server.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

test("each continue is another single-call tick; silence and duplicate events do not restart it", async () => {
  const h = await fixture(async (_request, call) =>
    call < 3
      ? { ...wait, next: { kind: "continue" } }
      : { ...wait, messages: [{ text: "done" }] },
  );
  try {
    h.server.addMessage("@helper work");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["done"],
    );
    await h.agent.requestSync();
    await h.restart();
    assert.equal(h.requests.length, 3);
    assert.match(
      h.requests[0].messages[0].content,
      /Every tick is one model call/,
    );
  } finally {
    await h.close();
  }
});

test("fast tool completion wakes on_jobs once and survives a tick boundary", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "save",
            tool: "memory",
            arguments: { action: "set", key: "index", value: "remembered" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["save"], mode: "all" },
      };
    assert.ok(
      request.messages.some((message) =>
        message.content.includes('"status":"completed"'),
      ),
    );
    return { ...wait, messages: [{ text: "saved" }] };
  });
  try {
    h.server.addMessage("@helper remember");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.equal(h.requests.length, 2);
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].observed,
      true,
    );
    await h.restart();
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
});

test("wait lets a job finish silently without triggering inference", async () => {
  const h = await fixture(async () => ({
    ...wait,
    actions: [
      {
        id: "note",
        tool: "memory",
        arguments: { action: "set", key: "index", value: "quiet" },
      },
    ],
  }));
  try {
    h.server.addMessage("@helper remember quietly");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.status === "completed",
      ),
    );
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    await h.close();
  }
});

test("delayed continuation survives restart and a new input supersedes a stale timer", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1 ? { ...wait, next: { kind: "after", delayMs: 150 } } : wait,
  );
  try {
    h.server.addMessage("@helper later");
    await h.agent.requestSync();
    await h.restart();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper another request");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
  } finally {
    await h.close();
  }
});

test("self ticks consume an episode budget and local controls stay outside inference", async () => {
  const h = await fixture(async () => ({
    ...wait,
    next: { kind: "continue" },
  }));
  try {
    h.config.set("agent.maxTicks", 3);
    h.server.addMessage("@helper continue");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.equal(h.store.state.sessions["conv-1"].phase, "failed");
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.equal(h.server.state.sends.at(-1)?.text, "Pong.");
  } finally {
    await h.close();
  }
});

test("new input during inference waits for the next tick without overlapping requests", async () => {
  let release: (() => void) | undefined;
  let active = 0;
  let maximum = 0;
  const h = await fixture(async (_request, call) => {
    active++;
    maximum = Math.max(maximum, active);
    if (call === 1)
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    active--;
    return wait;
  });
  try {
    h.server.addMessage("@helper first");
    await h.agent.requestSync(true, false);
    await until(() => release !== undefined);
    h.server.addMessage("@helper second");
    await h.agent.requestSync(true, false);
    assert.equal(h.requests.length, 1);
    release!();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(maximum, 1);
    assert.equal(h.requests[1].messages.at(-1)?.content, "@helper second");
  } finally {
    release?.();
    await h.close();
  }
});

test("a malformed decision starts no effects and unknown job watches are rejected", () => {
  assert.throws(() =>
    tickDecision(
      {
        ...wait,
        actions: [
          {
            id: "save",
            tool: "memory",
            arguments: { action: "set", value: 1 },
          },
          { id: "bad", tool: "unavailable", arguments: {} },
        ],
      },
      [MEMORY_TOOL],
    ),
  );
  assert.throws(() =>
    tickDecision({ ...wait, next: { kind: "after", delayMs: -1 } }, []),
  );
});

test("a slow room does not block a DM or local controls", async () => {
  let release: (() => void) | undefined;
  const h = await fixture(async (request) => {
    if (request.contextKey === "conv-1") {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return wait;
    }
    return { ...wait, messages: [{ text: "DM answered" }] };
  });
  try {
    const me = h.api.me.bind(h.api);
    const history = h.api.history.bind(h.api);
    const commands = h.api.serverCommands.bind(h.api);
    const send = h.api.send.bind(h.api);
    const messageContext = h.api.messageContext.bind(h.api);
    h.api.me = async () => {
      const snapshot = await me();
      return {
        ...snapshot,
        conversations: [
          ...snapshot.conversations,
          {
            ...snapshot.conversations[0],
            id: "direct-2",
            view: "@direct:alice",
          },
        ],
      };
    };
    h.api.history = (query, signal) =>
      history(
        {
          ...query,
          conversation_id:
            query.conversation_id === "direct-2"
              ? "conv-1"
              : query.conversation_id,
        },
        signal,
      );
    h.api.serverCommands = (id) => commands(id === "direct-2" ? "conv-1" : id);
    h.api.send = (request, key) =>
      send(
        {
          ...request,
          conversation_id:
            request.conversation_id === "direct-2"
              ? "conv-1"
              : request.conversation_id,
        },
        key,
      );
    h.api.messageContext = (id, message) =>
      messageContext(id === "direct-2" ? "conv-1" : id, message);
    await h.agent.sync();
    h.server.addMessage("@helper question");
    await h.agent.requestSync(true, false);
    await until(
      () =>
        release !== undefined &&
        h.server.state.sends.some((send) => send.text === "DM answered"),
    );
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync(true, false);
    assert.ok(h.server.state.sends.some((send) => send.text === "Pong."));
    assert.equal(h.requests.length, 2);
    release!();
    await h.agent.requestSync();
  } finally {
    release?.();
    await h.close();
  }
});

test("new input replaces a job watch; the old completion cannot start an extra tick", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    if (call === 1) {
      request.executeTool = () =>
        new Promise<string>((resolve) => {
          release = resolve;
        });
      return {
        ...wait,
        actions: [
          {
            id: "slow",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["slow"], mode: "all" },
      };
    }
    return wait;
  });
  try {
    h.server.addMessage("@helper start");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    h.server.addMessage("@helper leave that result for later");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    release!("completed result");
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).every(
        (job) => job.status === "completed",
      ),
    );
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper now use it");
    await h.agent.requestSync();
    assert.ok(
      h.requests[2].messages.some((message) =>
        message.content.includes("completed result"),
      ),
    );
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("a superseded timer is ignored and no provider retries are hidden inside a tick", async (t) => {
  const h = await fixture(async (_request, call) =>
    call === 1 ? { ...wait, next: { kind: "after", delayMs: 75 } } : wait,
  );
  try {
    h.server.addMessage("@helper later");
    await h.agent.requestSync();
    h.server.addMessage("@helper now");
    await h.agent.requestSync();
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return new Response("unavailable", { status: 503 });
  });
  const model = new ModelClient({
    endpoint: "https://model.invalid",
    token: "",
    model: "model",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  await assert.rejects(model.tick({ messages: [] }), /503/);
  assert.equal(calls, 1);
  t.mock.restoreAll();
});

test("retraction interrupts a watched job and suppresses late publication", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    if (call !== 1) throw new Error("Retracted work must not tick again");
    request.executeTool = () =>
      new Promise<string>((resolve) => {
        release = resolve;
      });
    return {
      ...wait,
      actions: [
        {
          id: "slow",
          tool: "memory",
          arguments: { action: "get", key: "index" },
        },
      ],
      next: { kind: "on_jobs", jobs: ["slow"], mode: "all" },
    };
  });
  try {
    const input = h.server.addMessage("@helper start");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    h.server.retractMessage(input.id);
    await h.agent.requestSync();
    release!("late result");
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).every(
        (job) => job.status === "interrupted",
      ),
    );
    assert.equal(h.requests.length, 1);
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("invalid reply plans commit no jobs and repair on a separate tick", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          messages: [{ text: "invalid", replyTo: "unrelated-message" }],
          actions: [
            {
              id: "write",
              tool: "memory",
              arguments: {
                action: "set",
                key: "index",
                value: "must not be written",
              },
            },
          ],
          next: { kind: "wait" },
        }
      : wait,
  );
  try {
    h.server.addMessage("@helper work");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(Object.keys(h.store.state.sessions["conv-1"].jobs).length, 0);
    assert.equal(h.server.state.sends.length, 0);
    assert.ok(
      h.requests[1].messages.some((message) =>
        message.content.includes("Reply target must be an admitted input"),
      ),
    );
  } finally {
    await h.close();
  }
});

test("native effects have distinct keys across ticks and recover known receipts after restart", async () => {
  let lost = false;
  const h = await fixture(async (_request, call) =>
    call <= 2
      ? {
          ...wait,
          actions: [
            {
              id: "command",
              tool: "run_server_command",
              arguments: { command: "/whoami" },
            },
          ],
          next: { kind: "on_jobs", jobs: ["command"], mode: "all" },
        }
      : wait,
  );
  try {
    const execute = h.api.executeCommand.bind(h.api);
    h.api.executeCommand = async (...args) => {
      const result = await execute(...args);
      if (!lost) {
        lost = true;
        throw new Error("lost committed response");
      }
      return result;
    };
    h.server.addMessage("@helper identity twice");
    await h.agent.requestSync();
    await until(() => h.requests.length === 3);
    await h.agent.requestSync();
    assert.equal(h.server.state.commands.length, 2);
    assert.equal(h.server.commandReceipts.size, 2);
    await h.restart();
    assert.equal(h.requests.length, 3);
    const actions =
      h.store.task("conv-1", h.store.state.sessions["conv-1"].root)?.actions ??
      [];
    assert.equal(actions.length, 2);
    assert.notEqual(actions[0].key, actions[1].key);
    assert.ok(actions.every((action) => action.status === "done"));
  } finally {
    await h.close();
  }
});

test("KV v1 migration retains accepted input, uncertain keys and pending output", async () => {
  const dir = mkdtempSync(join(tmpdir(), "simplebot-migrate-"));
  const path = join(dir, "agent.kv");
  let store = await Store.open(path);
  try {
    store.state.version = 1;
    store.state.account = "account";
    store.setCursor("conversation", 42);
    store.putTask({
      conversation: "conversation",
      input: "input",
      sequence: 42,
      promptHash: "hash",
      replyTo: null,
      status: "running",
      attempts: 1,
      slots: ["answer"],
      updated: Date.now(),
      actions: [
        {
          command: "/whoami",
          key: "original-command-key",
          status: "pending",
          created: Date.now(),
        },
      ],
    });
    store.putPart({
      slot: "conversation:input:answer",
      key: "original-output-key",
      request: { conversation_id: "conversation", text: "saved output" },
      created: Date.now(),
    });
    await store.close();
    store = await Store.open(path);
    assert.equal(store.state.version, 2);
    assert.equal(store.cursor("conversation"), 42);
    assert.equal(
      store.task("conversation", "input")?.actions?.[0].key,
      "original-command-key",
    );
    assert.equal(
      store.part("conversation:input:answer")?.request.text,
      "saved output",
    );
    assert.equal(
      store.part("conversation:input:answer")?.key,
      "original-output-key",
    );
    assert.equal((await Store.inspect(path)).version, 2);
  } finally {
    await store.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a full inbox pauses scanning and resumes when ticks free capacity", async () => {
  const h = await fixture(async () => wait);
  try {
    h.config.set("agent.inboxLimit", 10);
    let latest = 0;
    for (let index = 0; index < 14; index++)
      latest = h.server.addMessage(`@helper input ${index}`).sequence;
    await h.agent.requestSync();
    await until(() => h.requests.length === 14);
    await h.agent.requestSync();
    assert.equal(h.store.cursor("conv-1"), latest);
    assert.ok(h.logs.some((line) => line.includes("inbox_full")));
    assert.ok(h.store.tasks().every((task) => task.status === "answered"));
  } finally {
    await h.close();
  }
});

test("token usage is charged across self continuations without resetting the episode", async () => {
  const h = await fixture(async (request) => {
    request.onCall?.({
      success: true,
      durationMs: 1,
      usage: { prompt: 500, completion: 600 },
    });
    return { ...wait, next: { kind: "continue" } };
  });
  try {
    h.config.set("agent.maxEpisodeTokens", 1024);
    h.server.addMessage("@helper budget");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.equal(h.store.state.sessions["conv-1"].tokens, 1100);
    assert.equal(h.store.state.sessions["conv-1"].phase, "failed");
  } finally {
    await h.close();
  }
});

test("large job results are excerpted and can be paged within their conversation", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1) {
      request.executeTool = async () => "result-".repeat(2000);
      return {
        ...wait,
        actions: [
          {
            id: "large",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["large"], mode: "all" },
      };
    }
    const stateMessage = request.messages.find((message) =>
      message.content.startsWith("Tick state (untrusted data"),
    )!;
    const state = JSON.parse(
      stateMessage.content.slice(stateMessage.content.indexOf("\n") + 1),
    ) as {
      jobs: {
        id: string;
        tool: string;
        result?: string;
        resultTruncated: boolean;
        observed: boolean;
      }[];
    };
    if (call === 2) {
      const result = state.jobs.find((job) => job.tool === "memory")!;
      assert.equal(result.result?.length, 4000);
      assert.equal(result.resultTruncated, true);
      assert.match(
        readJob(undefined, JSON.stringify({ job: result.id })),
        /Unknown job/,
      );
      return {
        ...wait,
        actions: [
          {
            id: "page",
            tool: "read_job",
            arguments: { job: result.id, offset: 5000, limit: 1000 },
          },
        ],
        next: { kind: "on_jobs", jobs: ["page"], mode: "all" },
      };
    }
    const page = JSON.parse(
      state.jobs.find((job) => job.tool === "read_job")!.result!,
    ) as { text: string; offset: number; nextOffset: number };
    assert.equal(page.text.length, 1000);
    assert.equal(page.offset, 5000);
    assert.equal(page.nextOffset, 6000);
    return wait;
  });
  try {
    h.server.addMessage("@helper large result");
    await h.agent.requestSync();
    await until(() => h.requests.length === 3);
    await h.agent.requestSync();
    assert.equal(h.store.state.sessions["conv-1"].phase, "idle");
    assert.ok(
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.result?.length === 14000,
      ),
    );
  } finally {
    await h.close();
  }
});

test("a committed job still dispatches if checkpoint completion crosses the inference deadline", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          ...wait,
          actions: [
            {
              id: "save",
              tool: "memory",
              arguments: {
                action: "set",
                key: "index",
                value: "saved after commit",
              },
            },
          ],
          next: { kind: "on_jobs", jobs: ["save"], mode: "all" },
        }
      : wait,
  );
  try {
    h.config.set("model.timeoutMs", 1000);
    const save = h.store.save.bind(h.store);
    let delayed = false;
    h.store.save = async () => {
      await save();
      if (!delayed && h.store.state.sessions["conv-1"]?.trace.length === 1) {
        delayed = true;
        await new Promise((resolve) => setTimeout(resolve, 1100));
      }
    };
    h.server.addMessage("@helper save");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].status,
      "completed",
    );
  } finally {
    await h.close();
  }
});

test("shell wait expiry returns control while a watched command outlives the model timeout", async () => {
  const h = await fixture(async (request, call) => {
    assert.ok(
      request.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "shell",
            tool: "run_shell",
            arguments: {
              command: "sleep 1.1; printf shell-result",
              waitMs: 10,
              timeoutMs: 3000,
            },
          },
        ],
        next: { kind: "on_jobs", jobs: ["shell"], mode: "all" },
      };
    const state = request.messages.find((message) =>
      message.content.startsWith("Tick state (untrusted data"),
    )!;
    assert.ok(state.content.includes("shell-result"));
    return { ...wait, messages: [{ text: "shell finished" }] };
  }, true);
  try {
    h.config.set("model.timeoutMs", 1000);
    h.server.addMessage("@helper shell command");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.waitExpired,
      ),
    );
    assert.equal(h.requests.length, 1);
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync();
    assert.ok(h.server.state.sends.some((send) => send.text === "Pong."));
    await until(() =>
      h.server.state.sends.some((send) => send.text === "shell finished"),
    );
    const job = Object.values(h.store.state.sessions["conv-1"].jobs)[0];
    assert.equal(JSON.parse(job.result!).ok, true);
  } finally {
    await h.close();
  }
});

test("disabled shell requests are rejected even when bypassing the model catalogue", async () => {
  let captured: CompletionRequest | undefined;
  const h = await fixture(async (request) => {
    captured = request;
    return wait;
  });
  try {
    h.server.addMessage("@helper hello");
    await h.agent.requestSync();
    assert.ok(
      !captured?.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    await assert.rejects(
      captured!.executeTool!("run_shell", '{"command":"printf bypass"}'),
      /--with-shell/,
    );
    h.config.set("prompt", "new personality");
    h.config.reload();
    assert.equal(h.agent.shell.enabled, false);
  } finally {
    await h.close();
  }
});

test("restart without --with-shell interrupts a shell job without replaying its command", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "shell",
            tool: "run_shell",
            arguments: {
              command:
                "printf started > started.txt; sleep 30; printf late > late.txt",
              waitMs: 0,
            },
          },
        ],
        next: { kind: "on_jobs", jobs: ["shell"], mode: "all" },
      };
    assert.ok(
      !request.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    assert.ok(
      request.messages.some((message) =>
        message.content.includes('"status":"interrupted"'),
      ),
    );
    return wait;
  }, true);
  try {
    h.server.addMessage("@helper long shell command");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.waitExpired,
      ),
    );
    await h.restart(false);
    assert.equal(h.requests.length, 2);
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].status,
      "interrupted",
    );
    assert.equal(h.agent.shell.enabled, false);
  } finally {
    await h.close();
  }
});

for (const stream of [false, true])
  test(`production adapter makes one provider call per tick (stream=${stream})`, async (t) => {
    let calls = 0;
    const decision = JSON.stringify(wait);
    t.mock.method(
      globalThis,
      "fetch",
      async (_url: unknown, init: RequestInit) => {
        calls++;
        const body = JSON.parse(String(init.body));
        assert.equal(body.tools.length, 1);
        assert.equal(body.tool_choice.function.name, "finish_tick");
        const tool = {
          id: "tick-1",
          type: "function",
          function: { name: "finish_tick", arguments: decision },
        };
        if (!stream)
          return new Response(
            JSON.stringify({
              choices: [
                {
                  message: { content: null, tool_calls: [tool] },
                  finish_reason: "tool_calls",
                },
              ],
            }),
          );
        return new Response(
          `data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, ...tool }] }, finish_reason: null }] })}\n\ndata: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "tool_calls" }] })}\n\ndata: [DONE]\n\n`,
          { headers: { "Content-Type": "text/event-stream" } },
        );
      },
    );
    const model = new ModelClient({
      endpoint: "https://model.invalid",
      token: "",
      model: "model",
      timeoutMs: 1000,
      stream,
      log: () => {},
    });
    assert.deepEqual(
      await model.tick({
        messages: [{ role: "user", content: "hello" }],
        tools: [MEMORY_TOOL],
      }),
      wait,
    );
    assert.equal(calls, 1);
    t.mock.restoreAll();
  });
