// Behavioural tests for the example worker.
//
//   node --test simpleBot/test/
//
// These run against the in-process mock API and a stub model, so they need no
// external chat server, external network access or credentials. The socket wiring is the
// same module the real entrypoint uses, so the live configuration path is
// covered too.
import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BotApi } from "../src/api.ts";
import { Agent } from "../src/agent.ts";
import { ConfigStore, DEFAULTS } from "../src/config.ts";
import { Store } from "../src/store.ts";
import { createSocketEvents } from "../src/wiring.ts";
import type { ChatModel, CompletionRequest } from "../src/model.ts";
import type { ServerFrame } from "../src/wire.ts";
import { MockServer } from "./mock-server.ts";

class StubModel implements ChatModel {
  readonly prompts: string[] = [];
  readonly requests: CompletionRequest[] = [];
  private readonly reply: string;

  constructor(reply = "stub answer") {
    this.reply = reply;
  }

  get describe(): string {
    return "stub-model";
  }
  async complete(request: CompletionRequest): Promise<string> {
    this.requests.push(request);
    this.prompts.push(request.messages.at(-1)?.content ?? "");
    return this.reply;
  }
}

interface Harness {
  server: MockServer;
  readonly agent: Agent;
  api: BotApi;
  store: Store;
  config: ConfigStore;
  model: StubModel;
  activities: string[];
  logs: string[];
  dir: string;
  /** Feed a server frame to the worker's socket wiring. */
  deliver(frame: ServerFrame): void;
  /** Wait until `check` passes, or fail with `what` after a timeout. */
  waitFor(
    what: string,
    check: () => boolean,
    timeoutMs?: number,
  ): Promise<void>;
  stop(): Promise<void>;
  restart(): Promise<void>;
}

/** Records the frames the worker would put on its socket. */
class FakeSocket {
  respondCommands(
    id: string,
    commands: import("../src/wire.ts").BotCommand[],
  ): void {
    this.server.receive({ kind: "commands_response", id, commands });
  }
  commandsChanged(): void {
    this.server.receive({ kind: "commands_changed" });
  }
  private readonly server: MockServer;

  constructor(server: MockServer) {
    this.server = server;
  }

  get connected(): boolean {
    return true;
  }
  respond(
    id: string,
    values: Record<string, string> | null,
    error: string | null,
  ): void {
    this.server.receive({ kind: "config_response", id, values, error });
  }
}

async function harness(
  options: { operations?: string[]; reply?: string } = {},
): Promise<Harness> {
  const server = new MockServer({ operations: options.operations ?? null });
  const url = await server.listen();
  const dir = mkdtempSync(join(tmpdir(), "simplebot-"));
  const config = ConfigStore.load(join(dir, "config.json"));
  config.set("chat.url", url);
  config.set("chat.token", server.token);
  config.set("chat.historyWindow", 0);
  let store = Store.load(join(dir, "checkpoint.json"));
  store.taskLimit = config.get("chat.stateLimit");
  const model = new StubModel(options.reply ?? "stub answer");
  const activities: string[] = [];
  const logs: string[] = [];
  const fake = new FakeSocket(server);

  let api = new BotApi({ baseUrl: url, token: server.token });
  let agent: Agent;
  const build = (): Agent =>
    new Agent({
      api,
      store,
      config,
      model,
      log: (level, message) => logs.push(`${level} ${message}`),
      presence: () => [],
      socketReady: () => true,
      setActivity: (_conversationId, state) => {
        activities.push(state);
      },
    });
  agent = build();

  const events = createSocketEvents({
    config,
    get store() {
      return store;
    },
    log: () => {},
    agent: () => agent,
    socket: () => fake,
    onTransportChange: () => {
      api = new BotApi({
        baseUrl: config.get("chat.url"),
        token: config.get("chat.token"),
      });
      agent = build();
    },
  });

  const waitFor = async (
    what: string,
    check: () => boolean,
    timeoutMs = 4000,
  ): Promise<void> => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (check()) return;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    throw new Error(`timed out waiting for ${what}`);
  };

  // Deliver the baseline snapshot the way the socket would on connect.
  await events.snapshot(server.snapshot());
  await waitFor("baseline snapshot", () => agent.directory !== null);

  return {
    server,
    get agent() {
      return agent;
    },
    get api() {
      return api;
    },
    get store() {
      return store;
    },
    config,
    model,
    activities,
    logs,
    dir,
    deliver(frame: ServerFrame) {
      switch (frame.kind) {
        case "snapshot":
          void events.snapshot(frame);
          return;
        case "conversation":
          events.conversation(frame.conversation_id, frame.content_revision);
          return;
        case "config_request":
          events.configRequest(frame);
          return;
        case "commands_request":
          events.commandsRequest(frame);
          return;
        case "presence":
          events.presence(frame.online);
          return;
        default:
          return;
      }
    },
    waitFor,
    async restart() {
      store = Store.load(join(dir, "checkpoint.json"));
      agent = build();
      await agent.handleSnapshot(server.snapshot());
    },
    async stop() {
      await server.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

test("baselines a first run at unread.through without replaying history", async () => {
  const h = await harness();
  try {
    h.server.addMessage("older chatter", { mentions: [] });
    const mention = h.server.addMessage("@helper hello");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 1, "exactly one reply");
    assert.match(h.server.state.sends[0]!.text, /stub answer/);
    assert.equal(h.server.state.sends[0]!.replyTo, mention.id);
    assert.equal(h.store.cursor(h.server.conversationId), mention.sequence);
    assert.deepEqual(h.model.prompts, ["@helper hello"]);
  } finally {
    await h.stop();
  }
});

test("a send is idempotent: replaying the same key returns the stored message", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper say something");
    await h.agent.handleSnapshot(h.server.snapshot());
    const first = h.server.state.sends[0]!;
    const replay = await h.api.send(
      { conversation_id: h.server.conversationId, text: first.text },
      first.key,
    );
    assert.equal(replay.replayed, true);
    assert.equal(
      h.server.state.sends.length,
      1,
      "no second send for the same key",
    );
  } finally {
    await h.stop();
  }
});

test("an edit detected by revision change produces an additive second pass", async () => {
  const h = await harness();
  try {
    const message = h.server.addMessage("@helper what is 2+2?");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 1);

    h.server.editMessage(message.id, "@helper what is 3+3?");
    await h.agent.handleSnapshot(h.server.snapshot());

    assert.equal(
      h.server.state.sends.length,
      2,
      "one original answer plus one revision answer",
    );
    assert.equal(h.server.state.sends[0]!.key, `conv-1:${message.id}:answer`);
    assert.equal(h.server.state.sends[1]!.key, `conv-1:${message.id}:edit:1`);
    assert.match(h.model.prompts.at(-1) ?? "", /edited/);
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.status,
      "answered",
    );
  } finally {
    await h.stop();
  }
});

test("an edit arriving during an in-flight answer is not lost", async () => {
  const h = await harness();
  try {
    const message = h.server.addMessage("@helper first version");
    await h.agent.handleSnapshot(h.server.snapshot());
    // Change the input while the (already delivered) pass is the recorded one.
    h.server.editMessage(message.id, "@helper second version");
    await h.agent.handleSnapshot(h.server.snapshot());
    await h.agent.requestSync();
    assert.equal(h.server.state.sends.length, 2);
    assert.match(h.model.prompts.at(-1) ?? "", /second version/);
  } finally {
    await h.stop();
  }
});

test("a retraction cancels unsent output and records the retraction", async () => {
  const h = await harness();
  try {
    const message = h.server.addMessage("@helper this will vanish");
    await h.agent.handleSnapshot(h.server.snapshot());
    const before = h.server.state.sends.length;
    h.server.retractMessage(message.id);
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.status,
      "cancelled",
    );
    assert.equal(
      h.server.state.sends.length,
      before,
      "no additional output after retraction",
    );
    assert.equal(h.store.metrics.retractions, 1);
  } finally {
    await h.stop();
  }
});

test("mention-prefixed server commands use model tools under the bot identity", async () => {
  const h = await harness();
  try {
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      assert.ok(
        request.tools?.some(
          (tool) => tool.function.name === "run_server_command",
        ),
      );
      assert.equal(
        await request.executeTool?.(
          "run_server_command",
          JSON.stringify({ command: "/whoami" }),
        ),
        JSON.stringify({ result: "Server command completed" }),
      );
      return complete(request);
    };
    h.server.addMessage("@helper identify your server account");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.deepEqual(h.server.state.commands, ["/whoami"]);
    assert.equal(h.model.requests.length, 1);
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.stop();
  }
});

test("an unknown slash command goes to the model, not to a handler", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper /explain the deployment");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.model.prompts.length, 1);
    assert.match(h.model.prompts[0]!, /\/explain the deployment/);
  } finally {
    await h.stop();
  }
});

test("long output is split into independently keyed numbered parts", async () => {
  const paragraphs =
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(120);
  const h = await harness({ reply: paragraphs });
  try {
    h.server.addMessage("@helper write a lot");
    await h.agent.handleSnapshot(h.server.snapshot());
    const sends = h.server.state.sends;
    assert.ok(sends.length > 1, `expected multiple parts, got ${sends.length}`);
    assert.match(sends[0]!.text, /^\[1\/\d+\] /);
    assert.match(sends[1]!.text, /^\[2\/\d+\] /);
    const keys = sends.map((send) => send.key);
    assert.equal(new Set(keys).size, keys.length, "every part has its own key");
    for (const send of sends) assert.ok(Array.from(send.text).length <= 4000);
  } finally {
    await h.stop();
  }
});

test("read acknowledgement is exposed only with operator opt-in", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper mark this read");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(
      !h.model.requests[0]?.tools?.some(
        (tool) => tool.function.name === "acknowledge_read",
      ),
    );
    assert.equal(h.server.state.reads.length, 0);
    h.config.set("chat.acknowledgeReads", true);
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      assert.ok(
        request.tools?.some(
          (tool) => tool.function.name === "acknowledge_read",
        ),
      );
      await request.executeTool?.("acknowledge_read", "{}");
      return complete(request);
    };
    const message = h.server.addMessage("@helper mark this read");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.deepEqual(h.server.state.reads, [{ through: message.sequence }]);
  } finally {
    await h.stop();
  }
});

test("a scoped credential without the send operation fails the task and surfaces it", async () => {
  const h = await harness({ operations: ["read"] });
  try {
    h.server.addMessage("@helper are you there?");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.server.state.sends.length,
      0,
      "no output without the send operation",
    );
    const task = h.store.tasks()[0];
    assert.equal(task?.status, "failed");
    assert.match(task?.error ?? "", /scope denied/i);
  } finally {
    await h.stop();
  }
});

test("configuration set/fetch round-trips over the socket and redacts secrets", async () => {
  const h = await harness();
  try {
    h.config.set("chat.token", "bot_supersecretvalue");
    h.deliver({
      kind: "config_request",
      id: "req-set",
      operation: "set",
      key: "chat.historyWindow",
      value: "12",
    });
    h.deliver({
      kind: "config_request",
      id: "req-get",
      operation: "fetch",
      key: "chat.token",
      value: null,
    });
    h.deliver({
      kind: "config_request",
      id: "req-all",
      operation: "fetch",
      key: null,
      value: null,
    });
    h.deliver({
      kind: "config_request",
      id: "req-bad",
      operation: "set",
      key: "nope.key",
      value: "x",
    });
    await h.waitFor(
      "config responses",
      () => h.server.state.configResponses.length >= 4,
    );
    const responses = h.server.state.configResponses;
    assert.deepEqual(
      responses.find((response) => response.id === "req-set")?.values,
      { "chat.historyWindow": "12" },
      "a set echoes exactly the key it changed",
    );
    assert.match(
      responses.find((response) => response.id === "req-get")!.values![
        "chat.token"
      ]!,
      /^<set:/,
    );
    assert.match(
      responses.find((response) => response.id === "req-all")!.values![
        "chat.token"
      ]!,
      /^<set:/,
      "a full fetch redacts the secret",
    );
    assert.match(
      responses.find((response) => response.id === "req-bad")!.error!,
      /unsupported key/,
    );
    assert.equal(h.config.get("chat.historyWindow"), 12);
  } finally {
    await h.stop();
  }
});

test("a rejected configuration value leaves the setting untouched", async () => {
  const h = await harness();
  try {
    h.deliver({
      kind: "config_request",
      id: "req-bad-value",
      operation: "set",
      key: "model.timeoutMs",
      value: "soon",
    });
    await h.waitFor(
      "rejection",
      () => h.server.state.configResponses.length >= 1,
    );
    const response = h.server.state.configResponses[0]!;
    assert.equal(response.values, null);
    assert.match(response.error!, /timeout must be an integer/);
    assert.equal(h.config.get("model.timeoutMs"), 120_000);
  } finally {
    await h.stop();
  }
});

test("the checkpoint round-trips through disk and refuses an unknown version", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper persist me");
    await h.agent.handleSnapshot(h.server.snapshot());
    const raw = JSON.parse(
      readFileSync(join(h.dir, "checkpoint.json"), "utf8"),
    ) as {
      version: number;
      cursors: Record<string, number>;
    };
    assert.equal(raw.version, 2);
    assert.equal(
      raw.cursors[h.server.conversationId],
      h.store.cursor(h.server.conversationId),
    );
    const reloaded = Store.load(join(h.dir, "checkpoint.json"));
    assert.equal(
      reloaded.cursor(h.server.conversationId),
      h.store.cursor(h.server.conversationId),
    );

    // A version-1 checkpoint must be refused rather than silently upgraded.
    const path = join(h.dir, "v1.json");
    writeFileSync(path, JSON.stringify({ ...raw, version: 1 }));
    assert.throws(() => Store.load(path), /not supported/);
  } finally {
    await h.stop();
  }
});

test("a checkpoint from another account is refused", async () => {
  const h = await harness();
  try {
    h.store.state.account = "someone-else";
    await assert.rejects(
      () => h.agent.handleSnapshot(h.server.snapshot()),
      /belongs to account/,
    );
  } finally {
    await h.stop();
  }
});

test("working and idle activity is emitted around a model call", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper do work");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(h.activities.includes("working"));
    assert.ok(h.activities.includes("idle"));
  } finally {
    await h.stop();
  }
});

test("a mention-free room message is not answered", async () => {
  const h = await harness();
  try {
    h.server.addMessage("just chatting", { mentions: [] });
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 0);
    assert.equal(
      h.store.cursor(h.server.conversationId),
      h.server.snapshot().conversations[0]!.unread!.through,
    );
  } finally {
    await h.stop();
  }
});

test("the checkpoint stays bounded as tasks accumulate", async () => {
  const h = await harness();
  try {
    // A small bound makes the retention behaviour observable.
    h.config.set("chat.stateLimit", 20);
    h.store.taskLimit = 20;
    for (let index = 0; index < 60; index++) {
      h.server.addMessage(`@helper message ${index}`);
      await h.agent.handleSnapshot(h.server.snapshot());
    }
    assert.ok(h.server.state.sends.length >= 60, "every mention was answered");
    const tasks = h.store.tasks().length;
    assert.ok(
      tasks <= 20,
      `task records are bounded by chat.stateLimit, got ${tasks}`,
    );
    const parts = Object.keys(h.store.state.parts).length;
    assert.ok(parts <= 20, `output records are bounded too, got ${parts}`);
    const onDisk = JSON.parse(
      readFileSync(join(h.dir, "checkpoint.json"), "utf8"),
    ) as { tasks: Record<string, unknown> };
    assert.ok(
      Object.keys(onDisk.tasks).length <= 20,
      "the file itself is bounded",
    );
  } finally {
    await h.stop();
  }
});

test("idle activity always names a real conversation", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper hello");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(h.activities.includes("idle"), "the busy period ends with idle");
    for (const activity of h.server.state.activities) {
      assert.ok(
        activity.conversation_id.length > 0,
        "the server rejects an empty conversation_id",
      );
    }
  } finally {
    await h.stop();
  }
});

test("a deferred null unread entry does not reset the cursor", async () => {
  const h = await harness();
  try {
    // Revision-only change: no new message, so the cursor must not move.
    const message = h.server.addMessage("@helper first");
    await h.agent.handleSnapshot(h.server.snapshot());
    const cursor = h.store.cursor(h.server.conversationId);
    assert.equal(cursor, message.sequence, "the mention was consumed");

    h.server.editMessage(message.id, "@helper first, edited");
    // A null unread entry is a deferred conversation: no cursor movement and no
    // reconcile, even though the revision moved.
    h.server.unreadUnavailable = true;
    await h.agent.sync();
    assert.equal(
      h.store.cursor(h.server.conversationId),
      cursor,
      "an unavailable unread entry is deferred, not treated as zero",
    );

    h.server.unreadUnavailable = false;
    // When metadata recovers, reconciliation runs without consuming anything.
    // The cursor only moves to the latest sequence this worker produced while
    // answering; the revision converges on the server's.
    await h.agent.sync();
    assert.equal(
      h.store.revision(h.server.conversationId),
      h.server.state.revision,
    );
    assert.equal(
      h.store.cursor(h.server.conversationId),
      Math.max(...h.server.state.messages.map((message) => message.sequence)),
      "the cursor never jumps past retained history",
    );
  } finally {
    await h.stop();
  }
});

test("model instructions and output both use Markdown", async () => {
  const h = await harness({ reply: "**answer**" });
  try {
    h.server.addMessage("@helper format this");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(h.model.requests[0]!.messages[0]!.content, /using Markdown/);
    assert.equal(h.server.state.messages.at(-1)!.markdown, true);
  } finally {
    await h.stop();
  }
});

test("every repeated edit gets its own durable output", async () => {
  const h = await harness();
  try {
    const m = h.server.addMessage("@helper first");
    await h.agent.handleSnapshot(h.server.snapshot());
    for (const text of ["second", "third", "fourth"]) {
      h.server.editMessage(m.id, `@helper ${text}`);
      await h.agent.handleSnapshot(h.server.snapshot());
    }
    assert.equal(h.server.state.sends.length, 4);
    assert.equal(new Set(h.server.state.sends.map((s) => s.key)).size, 4);
  } finally {
    await h.stop();
  }
});

test("catch-up pages past parked inputs and finds a task in the middle of history", async () => {
  const h = await harness();
  try {
    const first = h.server.addMessage("@helper /ping");
    for (let i = 0; i < 120; i++)
      h.server.addMessage("room chatter", { mentions: [] });
    const middle = h.server.addMessage("@helper /whoami");
    for (let i = 0; i < 120; i++)
      h.server.addMessage("more chatter", { mentions: [] });
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.store.task(h.server.conversationId, first.id)?.status,
      "answered",
    );
    assert.equal(
      h.store.task(h.server.conversationId, middle.id)?.status,
      "answered",
    );
    assert.equal(h.server.state.sends.length, 2);
    assert.ok(h.store.cursor(h.server.conversationId)! >= middle.sequence);
  } finally {
    await h.stop();
  }
});

test("restart recovers running tasks without losing saved multipart output", async () => {
  const h = await harness({ reply: "x".repeat(8200) });
  try {
    const m = h.server.addMessage("@helper long answer");
    const original = h.api.send.bind(h.api);
    let calls = 0;
    h.api.send = async (input, key) => {
      if (++calls === 2) throw new Error("simulated crash before second send");
      return original(input, key);
    };
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 1);
    const task = h.store.task(h.server.conversationId, m.id)!;
    task.status = "running";
    h.store.save();
    h.api.send = original;
    h.model.complete = async () => {
      throw new Error("saved output must bypass the model");
    };
    await h.restart();
    assert.equal(h.server.state.sends.length, 3);
    assert.deepEqual(
      h.server.state.sends.map((s) => s.text.slice(0, 5)),
      ["[1/3]", "[2/3]", "[3/3]"],
    );
    assert.equal(
      h.store.task(h.server.conversationId, m.id)?.status,
      "answered",
    );
  } finally {
    await h.stop();
  }
});

test("expired pending output stops without issuing a fresh key", async () => {
  const h = await harness();
  try {
    const m = h.server.addMessage("@helper answer");
    const original = h.api.send.bind(h.api);
    h.api.send = async () => {
      throw new Error("network unavailable");
    };
    await h.agent.handleSnapshot(h.server.snapshot());
    const part = Object.values(h.store.state.parts)[0]!;
    part.created = Date.now() - 24 * 60 * 60 * 1000;
    h.store.save();
    h.api.send = original;
    await h.restart();
    assert.equal(h.server.state.sends.length, 0);
    assert.match(
      h.store.task(h.server.conversationId, m.id)?.error ?? "",
      /receipt expiry/,
    );
    assert.equal(Object.values(h.store.state.parts)[0]?.key, part.key);
    assert.ok(h.store.cursor(h.server.conversationId)! < m.sequence);
  } finally {
    await h.stop();
  }
});

test("retracting an input during a real in-flight model call suppresses its output", async () => {
  const h = await harness();
  try {
    const m = h.server.addMessage("@helper delayed");
    let release: (text: string) => void = () => {};
    let started = false;
    h.model.complete = () =>
      new Promise((resolve) => {
        release = resolve;
        started = true;
      });
    const pending = h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor("model started", () => started);
    h.server.retractMessage(m.id);
    await h.agent.sync();
    release("answer after retraction");
    await pending;
    assert.equal(h.server.state.sends.length, 0);
    assert.equal(
      h.store.task(h.server.conversationId, m.id)?.status,
      "cancelled",
    );
  } finally {
    await h.stop();
  }
});

test("retracting a stalled model request aborts it and releases the next queued mention", async () => {
  const h = await harness();
  try {
    let started = false;
    let aborted = false;
    let calls = 0;
    h.model.complete = (request) => {
      calls++;
      if (calls > 1) return Promise.resolve("next answer");
      started = true;
      return new Promise((_resolve, reject) => {
        request.signal!.addEventListener(
          "abort",
          () => {
            aborted = true;
            reject(new Error("cancelled fixture request"));
          },
          { once: true },
        );
      });
    };
    const first = h.server.addMessage("@helper stalled request");
    const running = h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor("stalled model started", () => started);
    h.server.retractMessage(first.id);
    const next = h.server.addMessage("@helper next request");
    await h.agent.sync();
    await running;
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(aborted, true);
    assert.equal(calls, 2);
    assert.equal(
      h.store.task(h.server.conversationId, first.id)?.status,
      "cancelled",
    );
    assert.equal(
      h.store.task(h.server.conversationId, next.id)?.status,
      "answered",
    );
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.server.state.sends[0]!.text, "next answer");
  } finally {
    await h.stop();
  }
});

test("pruning preserves pending parts with colon-containing conversation IDs", async () => {
  const h = await harness();
  try {
    const task = {
      conversation: "room:opaque",
      input: "message:opaque",
      sequence: 1,
      promptHash: "hash",
      replyTo: null,
      status: "failed" as const,
      attempts: 1,
      slots: ["answer"],
      updated: 0,
    };
    h.store.putTask(task);
    const slot = "room:opaque:message:opaque:answer";
    h.store.putPart({
      slot,
      key: "pending-key",
      request: { conversation_id: task.conversation, text: "saved" },
      created: Date.now(),
    });
    h.store.prune(1);
    assert.equal(h.store.part(slot)?.key, "pending-key");
    assert.throws(
      () => h.store.forget(new Set([h.server.conversationId])),
      /pending output target/,
    );
  } finally {
    await h.stop();
  }
});

test("stopping a busy agent preserves its input for the replacement transport", async () => {
  const h = await harness();
  try {
    const m = h.server.addMessage("@helper delayed model");
    let started = false;
    h.model.complete = (request) =>
      new Promise((_resolve, reject) => {
        started = true;
        request.signal?.addEventListener(
          "abort",
          () => reject(new Error("aborted")),
          { once: true },
        );
      });
    const pending = h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor("in-flight model", () => started);
    await h.agent.stop();
    await pending;
    assert.equal(h.store.task(h.server.conversationId, m.id)?.status, "queued");
    assert.equal(h.server.state.sends.length, 0);
    h.model.complete = async () => "resumed answer";
    await h.restart();
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.server.state.sends[0]!.text, "resumed answer");
  } finally {
    await h.stop();
  }
});

test("unchanged directory snapshots avoid redundant history reads", async () => {
  const h = await harness();
  try {
    const before = h.server.state.historyCalls;
    await h.agent.handleSnapshot(h.server.snapshot());
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.historyCalls, before);
  } finally {
    await h.stop();
  }
});

test("corrupt checkpoint nested records fail closed", async () => {
  const h = await harness();
  try {
    const checkpoint = JSON.parse(readFileSync(h.store.path, "utf8")) as Record<
      string,
      unknown
    >;
    checkpoint.account = 42;
    writeFileSync(h.store.path, JSON.stringify(checkpoint));
    assert.throws(() => Store.load(h.store.path), /account is invalid/);
    checkpoint.account = h.server.account.id;
    checkpoint.parts = { corrupt: { request: null } };
    writeFileSync(h.store.path, JSON.stringify(checkpoint));
    assert.throws(
      () => Store.load(h.store.path),
      /checkpoint output is invalid/,
    );
  } finally {
    await h.stop();
  }
});

test("existing v2 checkpoints adopt legacy output slots before the next edit", async () => {
  const h = await harness();
  try {
    const m = h.server.addMessage("@helper initial");
    await h.agent.handleSnapshot(h.server.snapshot());
    const task = h.store.task(h.server.conversationId, m.id)!;
    delete task.outputSlot;
    delete task.outputHash;
    delete task.pass;
    h.store.save();
    await h.restart();
    assert.equal(h.server.state.sends.length, 1);
    h.server.editMessage(m.id, "@helper edited after upgrade");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 2);
    assert.match(h.model.prompts.at(-1) ?? "", /edited after upgrade/);
  } finally {
    await h.stop();
  }
});

test("model output budget and reasoning effort are configurable and validated", async () => {
  const h = await harness();
  try {
    assert.equal(h.config.get("model.maxTokens"), 4096);
    assert.equal(h.config.get("model.reasoningEffort"), "");
    h.config.apply("model.maxTokens", "8192");
    h.config.apply("model.reasoningEffort", "none");
    assert.throws(
      () => h.config.apply("model.maxTokens", "0"),
      /output token limit/,
    );
    assert.throws(
      () => h.config.apply("model.reasoningEffort", "mystery"),
      /reasoning effort/,
    );
    h.server.addMessage("@helper explain retry receipts");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.model.requests[0]?.maxTokens, 8192);
    assert.equal(h.model.requests[0]?.reasoningEffort, "none");
  } finally {
    await h.stop();
  }
});

test("model knows server grants and never receives worker administration or legacy diagnostic history", async () => {
  const h = await harness();
  try {
    h.config.set("chat.historyWindow", 20);
    h.server.addMessage("@helper /model");
    h.server.addMessage("@helper what can you run?");
    await h.agent.handleSnapshot(h.server.snapshot());
    const system = h.model.requests.at(-1)!.messages[0]!.content;
    assert.match(system, /run_server_command/);
    assert.match(system, /\/new room name/);
    for (const privateName of [
      "worker.status",
      "worker.model",
      "/features",
      "/newroom",
      "/model",
      "no tool access",
    ])
      assert.ok(
        !h.model.requests
          .at(-1)!
          .messages.some((message) => message.content.includes(privateName)),
        privateName,
      );
    const request = h.model.requests.at(-1)!;
    assert.ok(
      request.tools?.some((tool) => tool.function.name === "read_history"),
    );
    const history = await request.executeTool!("read_history", "{}");
    assert.ok(!history.includes("/model"));
    assert.ok(
      !history.includes("what can you run?"),
      "the current input is not duplicated as history",
    );
    for (const command of ["/history", "/sudo /history"]) {
      assert.match(
        await request.executeTool!(
          "run_server_command",
          JSON.stringify({ command }),
        ),
        command === "/history"
          ? /read_history/
          : /not in the available server catalogue/,
      );
    }
    assert.equal(h.server.state.commands.length, 0);
    assert.match(
      await request.executeTool!("read_history", '{"limit":21}'),
      /bounded/,
    );
    h.deliver({
      kind: "config_request",
      id: "diagnostics",
      operation: "fetch",
      key: "worker.model",
      value: null,
    });
    assert.match(
      h.server.state.configResponses.at(-1)!.values?.["worker.model"] ?? "",
      /Model: stub-model/,
    );
  } finally {
    await h.stop();
  }
});

test("restricted credentials have no server command tool or public maintenance catalogue", async () => {
  const h = await harness({ operations: ["read", "send"] });
  try {
    h.config.set("chat.respondToAllDms", false);
    h.server.addMessage("@helper what can you run?");
    await h.agent.handleSnapshot(h.server.snapshot());
    const request = h.model.requests[0]!;
    assert.match(
      request.messages[0]!.content,
      /must mention the bot in both rooms and DMs/,
    );
    assert.match(request.messages[0]!.content, /command grants: none/);
    assert.deepEqual(
      request.tools?.map((tool) => tool.function.name),
      ["read_history"],
    );
    assert.deepEqual(
      (await h.agent.commands(h.server.conversationId)).map(
        (command) => command.name,
      ),
      ["/metrics", "/help", "/ping"],
    );
  } finally {
    await h.stop();
  }
});

test("v1 discovery marks the bot private command catalogue as operator-only", async () => {
  const h = await harness();
  try {
    h.deliver({
      kind: "commands_request",
      id: "catalogue-1",
      conversation_id: h.server.conversationId,
    });
    await h.waitFor("command response", () =>
      h.server.frames.some((frame) => frame.kind === "commands_response"),
    );
    const response = h.server.frames.find(
      (frame) => frame.kind === "commands_response",
    );
    assert.ok(response?.kind === "commands_response");
    assert.deepEqual(
      response.commands,
      await h.agent.commands(h.server.conversationId),
    );
    assert.ok(response.commands.some((command) => command.name === "/whoami"));
    assert.ok(
      response.commands
        .filter(
          (command) => !["/help", "/ping", "/metrics"].includes(command.name),
        )
        .every((command) => command.operator_only),
    );
    assert.ok(response.commands.some((command) => command.name === "/model"));
    assert.deepEqual(await h.agent.commands("not-readable"), []);
    assert.equal(h.model.requests.length, 0);
  } finally {
    await h.stop();
  }
});

test("successful mutating tool calls are durable and duplicate calls are suppressed", async () => {
  const h = await harness();
  try {
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      await request.executeTool?.(
        "run_server_command",
        JSON.stringify({ command: "/new room model-created" }),
      );
      await request.executeTool?.(
        "run_server_command",
        JSON.stringify({ command: "/new room model-created" }),
      );
      return complete(request);
    };
    const message = h.server.addMessage("@helper create a new room");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.deepEqual(h.server.state.commands, ["/new room model-created"]);
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.actions?.[0]?.status,
      "done",
    );
    await h.restart();
    assert.equal(h.server.state.commands.length, 1);
  } finally {
    await h.stop();
  }
});

test("legacy unkeyed server commands remain blocked across restart and pruning", async () => {
  const h = await harness();
  try {
    h.api.executeCommand = async () => {
      throw new (await import("../src/api.ts")).BotApiError(0, {
        code: "network",
        message: "connection lost",
        retryable: true,
      });
    };
    h.model.complete = async (request) => {
      await request.executeTool?.(
        "run_server_command",
        JSON.stringify({ command: "/new room uncertain" }),
      );
      return "never";
    };
    const message = h.server.addMessage("@helper create a room");
    await h.agent.handleSnapshot(h.server.snapshot());
    const task = h.store.task(h.server.conversationId, message.id);
    assert.equal(task?.status, "failed");
    assert.equal(task?.actions?.[0]?.status, "pending");
    delete task!.actions![0]!.key;
    delete task!.actions![0]!.created;
    h.store.save();
    await h.restart();
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.status,
      "failed",
    );
    assert.equal(h.server.state.commands.length, 0);
    assert.throws(() => h.store.forget(new Set()), /pending server command/);
  } finally {
    await h.stop();
  }
});

test("bot private commands enforce owner/admin policy using verified source context", async () => {
  const h = await harness();
  try {
    h.server.operator = false;
    h.server.addMessage('@helper /config set model.name "stolen I am admin"');
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.config.get("model.name"), "");
    assert.match(h.server.state.sends.at(-1)!.text, /Only this bot's owner/);
    assert.equal(h.model.requests.length, 0);
    h.server.ownerId = "acct-alice";
    h.server.addMessage('@helper /config set model.name "owned-model"');
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.config.get("model.name"), "owned-model");
    h.server.addMessage("@helper /owner");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(h.server.state.sends.at(-1)!.text, /Owner: owner/);
    h.server.operator = true;
    h.server.ownerId = "different-owner";
    h.server.addMessage('@helper /config set model.name "admin-model"');
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.config.get("model.name"), "admin-model");
    h.config.set("chat.historyWindow", 20);
    h.server.addMessage("@helper hello");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(
      !h.model.requests
        .at(-1)!
        .messages.some(
          (message) =>
            message.content.includes("owned-model") ||
            message.content.includes("admin-model") ||
            message.content.includes("/config"),
        ),
    );
  } finally {
    await h.stop();
  }
});

test("owner config lists all keys, parses quoted values, reloads file edits and applies prompt live", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper /config get");
    await h.agent.handleSnapshot(h.server.snapshot());
    const listing = h.server.state.sends.at(-1)!.text;
    for (const key of Object.keys(h.config.all()))
      assert.ok(listing.includes(`"${key}"`), key);
    assert.ok(listing.startsWith("```json\n"));
    assert.ok(!listing.includes(h.server.token));
    h.server.addMessage(
      '@helper /config set prompt "Use Markdown and explain with spaces"',
    );
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.config.get("prompt"),
      "Use Markdown and explain with spaces",
    );
    h.server.addMessage("@helper /config set prompt unquoted text");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(h.server.state.sends.at(-1)!.text, /double-quoted/);
    assert.equal(
      h.config.get("prompt"),
      "Use Markdown and explain with spaces",
    );
    h.server.addMessage('@helper /config set model.token ""');
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.config.get("model.token"), "");
    const disk = JSON.parse(readFileSync(h.config.path, "utf8"));
    disk.prompt = "Reloaded custom instruction";
    disk["model.maxTokens"] = 8192;
    writeFileSync(h.config.path, JSON.stringify(disk));
    h.server.addMessage("@helper /reload");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(h.server.state.sends.at(-1)!.text, /Configuration reloaded/);
    assert.equal(h.config.get("prompt"), "Reloaded custom instruction");
    assert.equal(h.model.requests.length, 0);
    h.server.addMessage("@helper explain this");
    await h.agent.handleSnapshot(h.server.snapshot());
    const request = h.model.requests.at(-1)!;
    assert.equal(request.messages[0]!.role, "system");
    assert.ok(request.messages[0]!.content.includes("using Markdown"));
    assert.ok(
      !request.messages[0]!.content.includes("Reloaded custom instruction"),
    );
    assert.ok(
      request.messages.some(
        (message) =>
          message.role === "user" &&
          message.content.includes("Reloaded custom instruction"),
      ),
    );
    assert.equal(h.model.requests.at(-1)!.maxTokens, 8192);
    writeFileSync(h.config.path, "{ broken");
    h.server.addMessage("@helper /reload");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.config.get("prompt"), "Reloaded custom instruction");
    assert.ok(
      !h.server.state.sends.at(-1)!.text.includes("Configuration reloaded"),
    );
  } finally {
    await h.stop();
  }
});

test("regular users get help and ping without model calls or private controls", async () => {
  const h = await harness({ operations: ["read", "send"] });
  try {
    h.server.operator = false;
    for (const text of ["@helper /help", "@helper /ping"]) {
      h.server.addMessage(text);
      await h.agent.handleSnapshot(h.server.snapshot());
    }
    assert.equal(h.model.requests.length, 0);
    assert.equal(h.server.state.sends.at(-1)!.text, "Pong.");
    const catalogue = await h.agent.commands(h.server.conversationId);
    assert.deepEqual(
      catalogue.map((command) => command.name),
      ["/metrics", "/help", "/ping"],
    );
    assert.ok(!h.server.state.sends[0]!.text.includes("/config"));
  } finally {
    await h.stop();
  }
});

test("public metrics are numeric, scoped to this conversation and need no model call", async () => {
  const h = await harness({ operations: ["read", "send"] });
  try {
    h.server.operator = false;
    h.model.complete = async (request) => {
      request.onCall?.({
        success: true,
        durationMs: 25,
        usage: { prompt: 100, completion: 10, cached: 60 },
      });
      request.onCall?.({ success: true, durationMs: 35 });
      return "Answer.";
    };
    h.server.addMessage("@helper hello");
    await h.agent.handleSnapshot(h.server.snapshot());
    h.server.addMessage("@helper /metrics");
    await h.agent.handleSnapshot(h.server.snapshot());
    const report = h.server.state.sends.at(-1)!.text;
    assert.match(report, /Model attempts: 2/);
    assert.match(report, /Input tokens: 100 \(1\/2 attempts reported\)/);
    assert.match(report, /Cached input tokens: 60/);
    assert.match(report, /60\.0%/);
    assert.match(report, /compact at 24576 tokens; total 32768 tokens/);
    assert.ok(!report.includes("bot_test"));
    await h.restart();
    h.server.addMessage("@helper /metrics");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(
      h.server.state.sends.at(-1)!.text,
      /Cached input tokens: unknown/,
    );
  } finally {
    await h.stop();
  }
});

test("debug explains task selection and model preparation without exposing conversation content", async () => {
  const h = await harness();
  try {
    h.config.set("prompt", "private-personality-marker");
    const ignored = h.server.addMessage("private-chatter-marker", {
      mentions: [],
    });
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes(`input=${ignored.id}`) &&
          line.includes("reason=not_addressed"),
      ),
    );
    assert.equal(h.model.requests.length, 0);
    const question = h.server.addMessage("@helper private-question-marker");
    await h.agent.handleSnapshot(h.server.snapshot());
    const logs = h.logs.join("\n");
    assert.match(logs, /task queued/);
    assert.match(logs, /task started/);
    assert.match(logs, /model preparation started/);
    assert.match(logs, /model command catalogue ready/);
    assert.match(logs, /model history ready/);
    assert.match(logs, /model preparation finished/);
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes(`input=${question.id}`) &&
          line.includes("status=answered"),
      ),
    );
    for (const secret of [
      "private-personality-marker",
      "private-chatter-marker",
      "private-question-marker",
      h.server.token,
    ])
      assert.ok(!logs.includes(secret));
  } finally {
    await h.stop();
  }
});

test("default context includes the newest 50 messages and keeps instructions stable across inputs", async () => {
  const h = await harness();
  try {
    assert.equal(DEFAULTS["chat.historyWindow"], 50);
    assert.equal(DEFAULTS["chat.contextChars"], 131072);
    assert.equal(DEFAULTS["model.contextTokens"], 32768);
    assert.equal(DEFAULTS["model.compactTokens"], 24576);
    h.config.set("chat.historyWindow", DEFAULTS["chat.historyWindow"]);
    for (let i = 0; i < 60; i++)
      h.server.addMessage(`discussion item ${i}`, { mentions: [] });
    const first = h.server.addMessage("@helper summarize the discussion");
    await h.agent.handleSnapshot(h.server.snapshot());
    const request = h.model.requests.at(-1)!;
    const pages = request.messages.filter((message) =>
      message.content.startsWith("Conversation data"),
    );
    assert.equal(pages.length, 1);
    const page = JSON.parse(pages[0]!.content.split("\n").slice(1).join("\n"));
    assert.equal(page.messages.length, 50);
    assert.equal(page.messages[0][3], "discussion item 10");
    assert.equal(page.messages.at(-1)[3], "discussion item 59");
    assert.ok(
      request.messages.some((message) => message.content.includes(first.id)),
    );
    assert.ok(!request.messages[0]!.content.includes(first.id));
    const next = h.server.addMessage("@helper elaborate");
    await h.agent.handleSnapshot(h.server.snapshot());
    const followup = h.model.requests.at(-1)!;
    assert.equal(followup.messages[0]!.content, request.messages[0]!.content);
    assert.equal(
      followup.messages.find((message) =>
        message.content.startsWith("Conversation data"),
      )!.content,
      pages[0]!.content,
    );
    assert.ok(
      followup.messages.filter((message) =>
        message.content.startsWith("Conversation data"),
      ).length > 1,
    );
    assert.ok(
      followup.messages.some((message) => message.content.includes(next.id)),
    );
  } finally {
    await h.stop();
  }
});

test("measured input at the compact threshold rebuilds the next answer's history", async () => {
  const h = await harness();
  try {
    h.config.set("chat.historyWindow", 2);
    h.server.addMessage("retained background", { mentions: [] });
    h.model.complete = async (request) => {
      h.model.requests.push(request);
      request.onCall?.({
        success: true,
        durationMs: 1,
        usage: { prompt: 24576, completion: 1 },
      });
      return "answer";
    };
    h.server.addMessage("@helper first question");
    await h.agent.handleSnapshot(h.server.snapshot());
    h.server.addMessage("@helper second question");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.model.requests.length, 2);
    assert.ok(
      h.model.requests.every(
        (request) => request.contextKey === h.server.conversationId,
      ),
    );
    assert.ok(h.model.requests.every((request) => request.resetTokenBaseline));
    assert.equal(
      h.model.requests[1]!.messages.filter((message) =>
        message.content.startsWith("Conversation data"),
      ).length,
      1,
    );
    h.server.addMessage("@helper /metrics");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(
      h.server.state.sends.at(-1)!.text,
      /Latest request input\/output \(provider\): 24576 \/ 1 tokens/,
    );
  } finally {
    await h.stop();
  }
});

test("fresh bounded context loads a quoted older page and the model can fetch more without duplicates", async () => {
  const h = await harness();
  try {
    h.config.set("chat.historyWindow", 2);
    const older = h.server.addMessage("older launch decision: ship Tuesday", {
      mentions: [],
    });
    for (let i = 0; i < 12; i++)
      h.server.addMessage(`recent discussion ${i}`, { mentions: [] });
    const question = h.server.addMessage("@helper explain the quoted decision");
    question.reply = { id: older.id, from: older.from, text: older.text };
    await h.agent.handleSnapshot(h.server.snapshot());
    const request = h.model.requests.at(-1)!;
    assert.ok(
      request.messages.some((message) =>
        message.content.includes("ship Tuesday"),
      ),
    );
    assert.ok(
      !request.messages.some((message) =>
        message.content.includes("recent discussion 8"),
      ),
    );
    const page = JSON.parse(
      await request.executeTool!(
        "read_history",
        JSON.stringify({ around: older.sequence + 7, limit: 2 }),
      ),
    );
    assert.ok(
      page.messages.some((row: unknown[]) =>
        String(row[3]).includes("recent discussion"),
      ),
    );
    assert.equal(typeof page.has_before, "boolean");
    const repeat = JSON.parse(
      await request.executeTool!(
        "read_history",
        JSON.stringify({ around: older.sequence + 7, limit: 2 }),
      ),
    );
    assert.equal(repeat.already_loaded, true);
    assert.match(
      await request.executeTool!(
        "read_history",
        JSON.stringify({ before: 5, after: 1 }),
      ),
      /one cursor/,
    );
    assert.ok(
      request.messages[0]!.content.length < 1000,
      "fixed instructions and compact command usages fit under 1000 characters in this fixture",
    );
    assert.equal(request.maxToolCalls, 4);
    const calls = h.model.requests.length;
    await h.restart();
    assert.equal(
      h.model.requests.length,
      calls,
      "restart does not replay delivered input",
    );
    h.server.editMessage(older.id, "older launch decision: ship Friday");
    const next = h.server.addMessage("@helper explain this refreshed quote");
    next.reply = { id: older.id, from: older.from, text: older.text };
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(
      h.model.requests
        .at(-1)!
        .messages.some((message) => message.content.includes("ship Friday")),
    );
    assert.ok(
      !h.model.requests
        .at(-1)!
        .messages.some((message) => message.content.includes("ship Tuesday")),
    );
  } finally {
    await h.stop();
  }
});

test("keyed pending commands recover a lost committed result after restart without another effect", async () => {
  const h = await harness();
  try {
    const execute = h.api.executeCommand.bind(h.api);
    h.api.executeCommand = async (...args) => {
      await execute(...args);
      throw new (await import("../src/api.ts")).BotApiError(0, {
        code: "network",
        message: "lost committed response",
        retryable: true,
      });
    };
    h.model.complete = async (request) => {
      await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/new room committed" }),
      );
      return "Created.";
    };
    const source = h.server.addMessage("@helper create a room");
    await h.agent.handleSnapshot(h.server.snapshot());
    const task = h.store.task(h.server.conversationId, source.id)!;
    assert.equal(task.status, "failed");
    assert.equal(task.actions![0]!.status, "pending");
    assert.equal(h.server.state.commands.length, 1);
    await h.restart();
    assert.equal(h.server.state.commands.length, 1);
    assert.equal(
      h.store.task(h.server.conversationId, source.id)?.status,
      "answered",
    );
    assert.equal(
      h.store.task(h.server.conversationId, source.id)?.actions?.[0]?.status,
      "done",
    );
  } finally {
    await h.stop();
  }
});

test("expired pending command keys are never automatically submitted again", async () => {
  const h = await harness();
  try {
    h.api.executeCommand = async () => {
      throw new (await import("../src/api.ts")).BotApiError(0, {
        code: "network",
        message: "lost",
        retryable: true,
      });
    };
    h.model.complete = async (request) => {
      await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/new room expired" }),
      );
      return "never";
    };
    const source = h.server.addMessage("@helper create a room");
    await h.agent.handleSnapshot(h.server.snapshot());
    const task = h.store.task(h.server.conversationId, source.id)!;
    task.actions![0]!.created = Date.now() - 24 * 60 * 60 * 1000;
    h.store.save();
    await h.restart();
    assert.equal(h.server.state.commands.length, 0);
    assert.match(
      h.store.task(h.server.conversationId, source.id)?.error ?? "",
      /window expired/,
    );
  } finally {
    await h.stop();
  }
});

test("receipt recovery resolves committed effects even after their source input disappears", async () => {
  const h = await harness();
  try {
    const execute = h.api.executeCommand.bind(h.api);
    h.api.executeCommand = async (...args) => {
      await execute(...args);
      throw new (await import("../src/api.ts")).BotApiError(0, {
        code: "network",
        message: "lost",
        retryable: true,
      });
    };
    h.model.complete = async (request) => {
      await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/new room retained-effect" }),
      );
      return "never";
    };
    const source = h.server.addMessage("@helper create this room");
    await h.agent.handleSnapshot(h.server.snapshot());
    h.server.retractMessage(source.id);
    await h.restart();
    const task = h.store.task(h.server.conversationId, source.id)!;
    assert.equal(task.actions![0]!.status, "done");
    assert.equal(task.status, "cancelled");
    assert.equal(h.server.state.commands.length, 1);
  } finally {
    await h.stop();
  }
});

test("restart resumes from a known command result when the model answer was interrupted", async () => {
  const h = await harness();
  try {
    h.model.complete = async (request) => {
      await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/new room done" }),
      );
      throw new Error("model answer interrupted");
    };
    const source = h.server.addMessage("@helper create this room");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.store.task(h.server.conversationId, source.id)?.actions?.[0]?.status,
      "done",
    );
    assert.equal(
      h.store.task(h.server.conversationId, source.id)?.status,
      "failed",
    );
    h.model.complete = async (request) => {
      assert.ok(
        request.messages.some(
          (message) =>
            message.role === "tool" &&
            message.content.includes("Server command completed"),
        ),
      );
      return "Recovered the completed action.";
    };
    await h.restart();
    assert.equal(h.server.state.commands.length, 1);
    assert.equal(
      h.store.task(h.server.conversationId, source.id)?.status,
      "answered",
    );
  } finally {
    await h.stop();
  }
});
