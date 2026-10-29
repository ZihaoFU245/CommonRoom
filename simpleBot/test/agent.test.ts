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
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BotApi } from "../src/api.ts";
import { Agent } from "../src/agent.ts";
import { ConfigStore, DEFAULTS } from "../src/config.ts";
import { STATE_VERSION, Store } from "../src/store.ts";
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
    this.server.receive({
      kind: "commands_response",
      id,
      commands,
      expires_in: 120,
    });
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
  let store = await Store.open(join(dir, "agent.kv"));
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
      workspace: join(dir, "workspace"),
      log: (level, message) => {
        logs.push(`${level} ${message}`);
        if (process.env.BOT_TEST_DEBUG === "1") console.error(`${level} ${message}`);
      },
      presence: () => [],
      socketReady: () => true,
      setActivity: (_conversationId, state) => {
        activities.push(state);
      },
    });
  agent = build();

  const events = createSocketEvents({
    get store() {
      return store;
    },
    log: () => {},
    agent: () => agent,
    socket: () => fake,
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
      await agent.stop();
      await store.close();
      store = await Store.open(join(dir, "agent.kv"));
      agent = build();
      await agent.handleSnapshot(server.snapshot());
    },
    async stop() {
      await agent.stop();
      await store.close();
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

test("owner configuration uses chat commands, validates values and redacts secrets", async () => {
  const h = await harness();
  try {
    h.server.addMessage('@helper /config set chat.historyWindow "12"');
    await h.agent.requestSync();
    assert.equal(h.config.get("chat.historyWindow"), 12);
    h.server.addMessage('@helper /config set model.timeoutMs "soon"');
    await h.agent.requestSync();
    assert.match(
      h.server.state.sends.at(-1)!.text,
      /timeout must be an integer/,
    );
    assert.equal(h.config.get("model.timeoutMs"), 120_000);
    h.server.addMessage("@helper /config get chat.token");
    await h.agent.requestSync();
    assert.match(h.server.state.sends.at(-1)!.text, /<set>/);
    assert.ok(!h.server.state.sends.at(-1)!.text.includes(h.server.token));
    assert.equal(h.model.requests.length, 0);
  } finally {
    await h.stop();
  }
});

test("worker state round-trips through KV and refuses an unknown version", async () => {
  const h = await harness();
  try {
    h.server.addMessage("@helper persist me");
    await h.agent.handleSnapshot(h.server.snapshot());
    const raw = await Store.inspect(h.store.path);
    assert.equal(raw.version, STATE_VERSION);
    assert.equal(
      raw.cursors[h.server.conversationId],
      h.store.cursor(h.server.conversationId),
    );
    await h.restart();
    assert.ok(
      h.store.cursor(h.server.conversationId)! >=
        raw.cursors[h.server.conversationId]!,
    );
    h.store.state.version = 999;
    await h.store.save();
    await assert.rejects(() => Store.open(h.store.path), /not supported/);
    h.store.state.version = STATE_VERSION;
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
    const onDisk = await Store.inspect(h.store.path);
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
    for (let i = 0; i < 120; i++) {
      h.server.addMessage("room chatter", { mentions: [] });
    }
    const middle = h.server.addMessage("@helper /whoami");
    for (let i = 0; i < 120; i++) {
      h.server.addMessage("more chatter", { mentions: [] });
    }
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
    await h.store.save();
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
    await h.store.save();
    h.api.send = original;
    await h.restart();
    assert.equal(h.server.state.sends.length, 0);
    assert.match(
      h.store.task(h.server.conversationId, m.id)?.error ?? "",
      /receipt expiry/,
    );
    assert.equal(Object.values(h.store.state.parts)[0]?.key, part.key);
    assert.equal(
      h.store.cursor(h.server.conversationId),
      m.sequence,
      "scan cursor records durable admission independently of delivery",
    );
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
    h.store.forget(new Set([h.server.conversationId]));
    assert.equal(h.store.part(slot)?.key, "pending-key");
    assert.ok(
      h.store.task(task.conversation, task.input),
      "inaccessible unresolved output remains recoverable",
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

test("corrupt durable records fail closed", async () => {
  const h = await harness();
  try {
    await h.store.records.commit([
      { field: "meta", id: "account", json: "42" },
    ]);
    await assert.rejects(() => Store.open(h.store.path), /account is invalid/);
    await h.store.records.commit([
      {
        field: "meta",
        id: "account",
        json: JSON.stringify(h.server.account.id),
      },
      {
        field: "parts",
        id: "corrupt",
        json: JSON.stringify({ request: null }),
      },
    ]);
    await assert.rejects(() => Store.open(h.store.path), /output is invalid/);
    await h.store.records.commit([{ field: "parts", id: "corrupt" }]);
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
    const system = h.model.requests
      .at(-1)!
      .messages.map((message) => message.content)
      .join("\n");
    assert.match(system, /run_server_command/);
    assert.match(system, /\/new room name/);
    for (const privateName of [
      "worker.status",
      "worker.model",
      "/features",
      "/newroom",
      "/model",
      "no tool access",
    ]) {
      assert.ok(
        !h.model.requests
          .at(-1)!
          .messages.some((message) => message.content.includes(privateName)),
        privateName,
      );
    }
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
          ? /Server command completed/
          : /not in the available server catalogue/,
      );
    }
    assert.deepEqual(h.server.state.commands, ["/history"]);
    assert.match(
      await request.executeTool!("read_history", '{"limit":21}'),
      /bounded/,
    );
    assert.match(
      h.agent.operatorValues()["worker.model"] ?? "",
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
    assert.ok(
      request.messages.some((message) =>
        /command grants: none/.test(message.content),
      ),
    );
    assert.deepEqual(
      request.tools?.map((tool) => tool.function.name),
      ["memory", "execute_code", "read_history"],
    );
    assert.deepEqual(
      (await h.agent.commands(h.server.conversationId)).map(
        (command) => command.name,
      ),
      ["/metrics", "/help", "/ping"],
    );
    assert.ok(
      h.logs.some(
        (line) =>
          line.startsWith("warn ") &&
          line.includes("server command catalogue unavailable") &&
          line.includes("status=403"),
      ),
      "a denied catalogue is reported instead of silently emptying the tool list",
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
    await h.store.save();
    await h.restart();
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.status,
      "failed",
    );
    assert.equal(h.server.state.commands.length, 0);
    h.store.forget(new Set());
    assert.equal(
      h.store.task(h.server.conversationId, message.id)?.actions?.[0]?.status,
      "pending",
    );
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
    for (const key of Object.keys(h.config.all())) {
      assert.ok(listing.includes(`"${key}"`), key);
    }
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
    assert.match(report, /Model attempts \| 2/);
    assert.match(report, /Input tokens \| 100 \(1\/2 attempts reported\)/);
    assert.match(report, /Cached input tokens \| 60/);
    assert.ok(report.includes("| Stage | Latest | Mean | P95 | Samples |"));
    assert.equal(h.server.state.messages.at(-1)?.markdown, true);
    assert.match(report, /End-to-end reply \| [^|]+ \| [^|]+ \| [^|]+ \| 1 \|/);
    assert.match(report, /Model calls per answered input \| 2\.00/);
    assert.match(report, /60\.0%/);
    assert.match(report, /compact at 24576 tokens; total 32768 tokens/);
    assert.ok(!report.includes("bot_test"));
    await h.restart();
    h.server.addMessage("@helper /metrics");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.match(
      h.server.state.sends.at(-1)!.text,
      /Cached input tokens \| unknown/,
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
    ]) {
      assert.ok(!logs.includes(secret));
    }
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
    for (let i = 0; i < 60; i++) {
      h.server.addMessage(`discussion item ${i}`, { mentions: [] });
    }
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
      /Latest request input\/output \| 24576 \/ 1 tokens/,
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
    for (let i = 0; i < 12; i++) {
      h.server.addMessage(`recent discussion ${i}`, { mentions: [] });
    }
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
      request.messages[0]!.content.length < 4000,
      "fixed instructions and compact command usages fit under 4000 characters in this fixture",
    );
    assert.equal(request.maxToolCalls, 32);
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
    await h.store.save();
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

test("one mention starts one turn with sender identity despite overlapping signals and restart", async () => {
  const h = await harness();
  try {
    const input = h.server.addMessage("@helper who am I?", {
      from: "小明",
      authorId: "acct-xiaoming",
    });
    await Promise.all([
      h.agent.requestSync(),
      h.agent.requestSync(),
      h.agent.handleSnapshot(h.server.snapshot()),
    ]);
    await h.agent.requestSync();
    await h.restart();
    assert.equal(h.model.requests.length, 1);
    assert.equal(h.server.state.sends.length, 1);
    assert.ok(
      h.model.requests[0]!.messages.some(
        (message) =>
          message.content.includes('"username":"小明"') &&
          message.content.includes('"author_id":"acct-xiaoming"'),
      ),
    );
    assert.equal(h.server.state.sends[0]!.replyTo, input.id);
  } finally {
    await h.stop();
  }
});

test("an agent can finish silently and the decision survives signals and restart", async () => {
  const h = await harness({ reply: "" });
  try {
    const input = h.server.addMessage("@helper no response needed");
    await h.agent.requestSync();
    await h.agent.requestSync();
    // Crash after the silent decision was saved but before terminal status.
    h.store.task(h.server.conversationId, input.id)!.status = "running";
    await h.store.save();
    await h.restart();
    assert.equal(h.server.state.sends.length, 0);
    assert.equal(h.model.requests.length, 1);
    assert.equal(
      h.store.task(h.server.conversationId, input.id)?.status,
      "answered",
    );
    assert.equal(h.store.cursor(h.server.conversationId), input.sequence);
  } finally {
    await h.stop();
  }
});

test("a tool action can complete without publishing a reply", async () => {
  const h = await harness({ reply: "" });
  try {
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/new room support" }),
      );
      return complete(request);
    };
    h.server.addMessage("@helper create support without a reply");
    await h.agent.requestSync();
    await h.restart();
    assert.deepEqual(h.server.state.commands, ["/new room support"]);
    assert.equal(h.model.requests.length, 1);
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    await h.stop();
  }
});

test("every granted server command is offered and executed, including sends", async () => {
  const h = await harness();
  try {
    const catalogue = h.api.serverCommands.bind(h.api);
    h.api.serverCommands = async (id) => ({
      ...(await catalogue(id)),
      commands: [
        { name: "/reply", usage: "/reply id text", description: "Reply" },
        { name: "/tell", usage: "/tell user text", description: "Send" },
        { name: "/sudo", usage: "/sudo command", description: "Elevate" },
      ],
    });
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      const system = request.messages
        .map((message) => message.content)
        .join("\n");
      for (const usage of ["/reply id text", "/tell user text"]) {
        assert.ok(system.includes(usage), usage);
      }
      for (const command of [
        "/reply m1 duplicate",
        "/tell alice duplicate",
        "/sudo /reply m1 duplicate",
        "/sudo /tell alice duplicate",
      ]) {
        assert.match(
          await request.executeTool!(
            "run_server_command",
            JSON.stringify({ command }),
          ),
          /Server command completed/,
        );
      }
      return complete(request);
    };
    h.server.addMessage("@helper hello");
    await h.agent.requestSync();
    assert.deepEqual(h.server.state.commands, [
      "/reply m1 duplicate",
      "/tell alice duplicate",
      "/sudo /reply m1 duplicate",
      "/sudo /tell alice duplicate",
    ]);
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.stop();
  }
});

test("a command outside the fetched catalogue is refused before any server effect", async () => {
  const h = await harness();
  try {
    let result = "";
    const complete = h.model.complete.bind(h.model);
    h.model.complete = async (request) => {
      result = await request.executeTool!(
        "run_server_command",
        JSON.stringify({ command: "/tell alice not granted" }),
      );
      return complete(request);
    };
    h.server.addMessage("@helper hello");
    await h.agent.requestSync();
    assert.match(result, /not in the available server catalogue/);
    assert.equal(h.server.state.commands.length, 0);
  } finally {
    await h.stop();
  }
});

test("an edit between queuing and locating is answered once for the located version", async () => {
  const h = await harness();
  try {
    const input = h.server.addMessage("@helper first");
    const history = h.api.history.bind(h.api);
    let edited = false;
    h.api.history = async (query) => {
      if (query.around === input.sequence && query.limit === 1 && !edited) {
        edited = true;
        h.server.editMessage(input.id, "@helper current");
      }
      return history(query);
    };
    await h.agent.requestSync();
    await h.agent.requestSync();
    assert.equal(h.model.requests.length, 1);
    assert.equal(h.model.prompts[0], "@helper current");
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.stop();
  }
});

test("a silent decision during an edit does not consume the newer input", async () => {
  const h = await harness();
  try {
    const input = h.server.addMessage("@helper stay silent");
    const complete = h.model.complete.bind(h.model);
    let first = true;
    h.model.complete = async (request) => {
      if (first) {
        first = false;
        h.server.editMessage(input.id, "@helper now answer");
        await h.agent.sync();
        return "";
      }
      return complete(request);
    };
    await h.agent.requestSync();
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(
      h.model.prompts[0],
      "The earlier version of this message was edited. Answer the current version:\n\n@helper now answer",
    );
    await h.restart();
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.stop();
  }
});

test("autonomous groups collect a fixed window of speakers into one plain response", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    h.config.set("chat.batchWindowMs", 3000);
    const first = h.server.addMessage("What is the plan?", {
      mentions: [],
      from: "alice",
    });
    let calls = 0;
    h.model.complete = async (request) => {
      calls++;
      const input = request.messages.at(-1)!.content;
      assert.ok(
        input.includes(first.id) &&
          input.includes("And when?") &&
          input.includes("bob"),
      );
      assert.match(request.messages[0]!.content, /Combine related questions/);
      assert.ok(
        request.tools?.some((tool) => tool.function.name === "finish_turn"),
      );
      assert.equal(request.maxToolCalls, 32);
      await request.executeTool!(
        "finish_turn",
        JSON.stringify({ messages: [{ text: "Ship on Friday." }] }),
      );
      return "This extra summary must not be sent";
    };
    const started = Date.now();
    const running = h.agent.requestSync();
    await new Promise((resolve) => setTimeout(resolve, 200));
    h.server.addMessage("And when?", {
      mentions: [],
      from: "bob",
      authorId: "acct-bob",
    });
    await h.agent.sync();
    await running;
    assert.ok(Date.now() - started >= 2900);
    assert.ok(
      Date.now() - started < 5000,
      "late messages do not reset the fixed window",
    );
    assert.equal(calls, 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => [send.text, send.replyTo]),
      [["Ship on Friday.", null]],
    );
    await h.agent.requestSync();
    await h.restart();
    assert.equal(
      calls,
      1,
      "own output and reconnect do not trigger a new decision",
    );
    assert.equal(h.store.state.version, STATE_VERSION);
  } finally {
    await h.stop();
  }
});

test("autonomous independent questions get separate replies in a single decision", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    const a = h.server.addMessage("What is 2+2?", { mentions: [] });
    const b = h.server.addMessage("Which day is launch?", {
      mentions: [],
      from: "bob",
      authorId: "acct-bob",
    });
    let calls = 0;
    h.model.complete = async (request) => {
      calls++;
      const bad = await request.executeTool!(
        "finish_turn",
        JSON.stringify({
          messages: [{ text: "bad", reply_to: "foreign-message" }],
        }),
      );
      assert.match(bad, /error/);
      await request.executeTool!(
        "finish_turn",
        JSON.stringify({
          messages: [
            { text: "4", reply_to: a.id },
            { text: "Friday", reply_to: b.id },
          ],
        }),
      );
      return "";
    };
    await h.agent.requestSync();
    assert.equal(calls, 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => [send.text, send.replyTo]),
      [
        ["4", a.id],
        ["Friday", b.id],
      ],
    );
    assert.ok(h.store.tasks().every((task) => task.status === "answered"));
  } finally {
    await h.stop();
  }
});

test("group turns start at least three seconds apart while new input joins the next batch", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    h.config.set("chat.batchWindowMs", 3000);
    h.server.addMessage("first", { mentions: [] });
    const starts: number[] = [];
    h.model.complete = async (request) => {
      starts.push(Date.now());
      if (starts.length === 1) {
        h.server.addMessage("follow up", { mentions: [] });
        await h.agent.sync();
      }
      await request.executeTool!("finish_turn", '{"messages":[]}');
      return "";
    };
    await h.agent.requestSync();
    assert.equal(starts.length, 2);
    assert.ok(starts[1]! - starts[0]! >= 2900);
    assert.equal(h.server.state.sends.length, 0);
    await h.restart();
    assert.equal(
      starts.length,
      2,
      "silent batches remain completed after restart",
    );
  } finally {
    await h.stop();
  }
});

test("autonomous mode uses owner chat controls and does not activate foreign private chats", async () => {
  const h = await harness();
  try {
    h.server.addMessage('@helper /config set autonomous "true"');
    await h.agent.requestSync();
    assert.equal(h.config.get("autonomous"), true);
    assert.equal(h.model.requests.length, 0);
    const me = h.api.me.bind(h.api);
    let privateChat = true;
    h.api.me = async () => {
      const snapshot = await me();
      snapshot.conversations[0]!.view = privateChat
        ? "@private:alice:bob"
        : "support";
      return snapshot;
    };
    h.server.addMessage("private discussion", { mentions: [] });
    await h.agent.requestSync();
    assert.equal(h.model.requests.length, 0);
    privateChat = false;
    h.server.addMessage('@helper /config set autonomous "false"');
    await h.agent.requestSync();
    h.server.addMessage("unmentioned chatter", { mentions: [] });
    await h.agent.requestSync();
    assert.equal(h.model.requests.length, 0);
  } finally {
    await h.stop();
  }
});

test("an upstream error releases the worker for follow-up questions", async () => {
  const h = await harness();
  try {
    const complete = h.model.complete.bind(h.model);
    let first = true;
    h.model.complete = async (request) => {
      if (first) {
        first = false;
        throw new Error("upstream failed");
      }
      return complete(request);
    };
    const failed = h.server.addMessage("@helper first question");
    h.server.addMessage("@helper follow-up question");
    await h.agent.requestSync();
    assert.equal(h.agent.busy, false);
    assert.equal(
      h.store.task(h.server.conversationId, failed.id)?.status,
      "failed",
    );
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.model.prompts[0], "@helper follow-up question");
  } finally {
    await h.stop();
  }
});

test("a model ignoring abort cannot hold the queue or run tools after its deadline", async () => {
  const h = await harness();
  try {
    h.config.set("model.timeoutMs", 1000);
    const complete = h.model.complete.bind(h.model);
    let abandoned: CompletionRequest | undefined;
    h.model.complete = async (request) => {
      if (!abandoned) {
        abandoned = request;
        return new Promise<string>(() => {});
      }
      return complete(request);
    };
    h.server.addMessage("@helper stalled request");
    h.server.addMessage("@helper next request");
    await h.agent.requestSync();
    assert.equal(h.agent.busy, false);
    assert.equal(h.server.state.sends.length, 1);
    await assert.rejects(
      abandoned!.executeTool!(
        "run_server_command",
        '{"command":"/new room late"}',
      ),
    );
    assert.equal(h.server.state.commands.length, 0);
  } finally {
    await h.stop();
  }
});

test("failed command receipt recovery cannot block new input", async () => {
  const h = await harness();
  try {
    const complete = h.model.complete.bind(h.model);
    h.api.executeCommand = async () => {
      throw new Error("uncertain upstream outcome");
    };
    h.api.commandReceipt = async () => {
      throw new Error("receipt service unavailable");
    };
    let first = true;
    h.model.complete = async (request) => {
      if (first) {
        first = false;
        await request.executeTool!(
          "run_server_command",
          '{"command":"/new room pending"}',
        );
      }
      return complete(request);
    };
    const failed = h.server.addMessage("@helper create room");
    await h.agent.requestSync();
    h.server.addMessage("@helper a new question");
    await h.agent.requestSync();
    assert.equal(h.agent.busy, false);
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(
      h.store.task(h.server.conversationId, failed.id)?.actions?.[0]?.status,
      "pending",
    );
  } finally {
    await h.stop();
  }
});

test("a saved multi-response group decision resumes without rerunning the model", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    const first = h.server.addMessage("a", { mentions: [] });
    const second = h.server.addMessage("b", { mentions: [] });
    let calls = 0;
    h.model.complete = async (request) => {
      calls++;
      await request.executeTool!(
        "finish_turn",
        JSON.stringify({
          messages: [
            { text: "first", reply_to: first.id },
            { text: "second", reply_to: second.id },
          ],
        }),
      );
      return "";
    };
    const send = h.api.send.bind(h.api);
    h.api.send = async (input, key) => {
      if (input.text === "second") throw new Error("send interrupted");
      return send(input, key);
    };
    await h.agent.requestSync();
    assert.equal(h.server.state.sends.length, 1);
    h.api.send = send;
    await h.restart();
    assert.equal(calls, 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["first", "second"],
    );
  } finally {
    await h.stop();
  }
});

test("retracted group input cancels its unsent decision and preserves the remaining question", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    const first = h.server.addMessage("remove this", { mentions: [] });
    const second = h.server.addMessage("keep this", { mentions: [] });
    let calls = 0;
    h.model.complete = async (request) => {
      calls++;
      if (calls === 1) {
        h.server.retractMessage(first.id);
        await h.agent.sync();
        await request.executeTool!(
          "finish_turn",
          '{"messages":[{"text":"stale"}]}',
        );
      }
      assert.ok(request.messages.at(-1)!.content.includes(second.id));
      return "current answer";
    };
    await h.agent.requestSync();
    assert.equal(calls, 2);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["current answer"],
    );
    assert.equal(
      h.store.task(h.server.conversationId, first.id)?.status,
      "cancelled",
    );
  } finally {
    await h.stop();
  }
});

test("sixteen distinct command effects survive checkpoint reload", async () => {
  const h = await harness();
  try {
    h.model.complete = async (request) => {
      for (let index = 0; index < 16; index++) {
        await request.executeTool!(
          "run_server_command",
          JSON.stringify({ command: `/new room demo${index}` }),
        );
      }
      return "";
    };
    h.server.addMessage("@helper do sixteen steps");
    await h.agent.requestSync();
    assert.equal(h.server.state.commands.length, 16);
    await h.restart();
    assert.equal(h.server.state.commands.length, 16);
    assert.equal(h.store.tasks()[0]?.actions?.length, 16);
  } finally {
    await h.stop();
  }
});

test("disabling autonomous mode cancels a waiting burst without a model call", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    const waiting = h.server.addMessage("ordinary discussion", {
      mentions: [],
    });
    const work = h.agent.requestSync();
    await h.waitFor(
      "waiting group task",
      () => !!h.store.task(h.server.conversationId, waiting.id),
    );
    h.server.addMessage('@helper /config set autonomous "false"');
    await h.agent.sync();
    await work;
    assert.equal(h.config.get("autonomous"), false);
    assert.equal(h.model.requests.length, 0);
    assert.equal(h.activities.at(-1), "idle");
    assert.equal(
      h.store.task(h.server.conversationId, waiting.id)?.status,
      "cancelled",
    );
  } finally {
    await h.stop();
  }
});

test("a failed group turn cannot pin later completed input or replay it after pruning", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    const failed = h.server.addMessage("first batch", { mentions: [] });
    let calls = 0;
    let nextSequence = 0;
    h.model.complete = async () => {
      calls++;
      if (calls === 1) {
        nextSequence = h.server.addMessage("follow-up batch", {
          mentions: [],
        }).sequence;
        await h.agent.sync();
        throw new Error("upstream temporarily failed");
      }
      return "recovered";
    };
    await h.agent.requestSync();
    assert.equal(calls, 2);
    assert.equal(
      h.store.task(h.server.conversationId, failed.id)?.status,
      "failed",
    );
    assert.ok((h.store.cursor(h.server.conversationId) ?? 0) >= nextSequence);
    h.store.prune(1);
    await h.agent.requestSync();
    assert.equal(calls, 2);
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.agent.busy, false);
  } finally {
    await h.stop();
  }
});

test("a long plain group response retains multipart delivery across checkpoint reload", async () => {
  const text = "x".repeat(20000);
  const h = await harness({ reply: text });
  try {
    h.config.set("autonomous", true);
    h.config.set("model.maxTokens", 8192);
    h.server.addMessage("please explain", { mentions: [] });
    await h.agent.requestSync();
    const count = h.server.state.sends.length;
    assert.ok(count > 1);
    assert.equal(
      h.server.state.sends
        .map((send) => send.text.replace(/^\[\d+\/\d+\] /u, ""))
        .join(""),
      text,
    );
    assert.ok(h.server.state.sends.every((send) => send.replyTo === null));
    await h.restart();
    assert.equal(h.server.state.sends.length, count);
    assert.equal(h.model.requests.length, 1);
  } finally {
    await h.stop();
  }
});

test("the agent can freely save memory silently and recall its index after restart", async () => {
  const h = await harness();
  try {
    h.model.complete = async (request) => {
      assert.ok(request.tools?.some((tool) => tool.function.name === "memory"));
      const result = JSON.parse(
        await request.executeTool!(
          "memory",
          JSON.stringify({
            action: "set",
            key: "index",
            value: {
              plan: "Compare Deno persistence options",
              status: "ongoing",
            },
          }),
        ),
      );
      assert.equal(result.ok, true);
      return "";
    };
    h.server.addMessage("@helper keep track of our storage investigation");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.length, 0);
    await h.restart();
    let recalled = false;
    h.model.complete = async (request) => {
      const bootstrap = request.messages.find((item) =>
        item.content.startsWith("Your persistent workspace"),
      );
      assert.match(
        bootstrap?.content ?? "",
        /Compare Deno persistence options/,
      );
      assert.match(bootstrap?.content ?? "", /ongoing/);
      recalled = true;
      return "We were comparing persistence options.";
    };
    h.server.addMessage("@helper where did we leave off?");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(recalled, true);
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.stop();
  }
});

test("a silent group can maintain shared memory with evidence from multiple speakers", async () => {
  const h = await harness();
  try {
    h.config.set("autonomous", true);
    h.model.complete = async (request) => {
      assert.match(request.messages[0]!.content, /independent speakers/);
      assert.equal(
        JSON.parse(
          await request.executeTool!(
            "memory",
            '{"action":"set","key":"index","value":{"proposal":"Deno","agreement":"unresolved"}}',
          ),
        ).ok,
        true,
      );
      await request.executeTool!("finish_turn", '{"messages":[]}');
      return "";
    };
    const first = h.server.addMessage("I propose Deno", {
      mentions: [],
      from: "alice",
      authorId: "acct-alice",
    });
    const second = h.server.addMessage("Let's compare it first", {
      mentions: [],
      from: "bob",
      authorId: "acct-bob",
    });
    await h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor(
      "shared memory saved",
      () =>
        h.store.task(h.server.conversationId, first.id)?.status === "answered",
      5000,
    );
    const entry = await h.store.records.kv.get<{
      sources: { id: string; authorId: string }[];
    }>(["workspace", h.server.account.id, h.server.conversationId, "index"]);
    assert.deepEqual(
      entry.value?.sources.map((source) => [source.id, source.authorId]),
      [
        [first.id, "acct-alice"],
        [second.id, "acct-bob"],
      ],
    );
    assert.equal(h.server.state.sends.length, 0);
    await h.restart();
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    await h.stop();
  }
});

test("the agent can execute KV source, save scripts and rerun shared workspace files after restart", async () => {
  const h = await harness({ operations: ["read", "send"] });
  try {
    const source =
      "const value: number = await Promise.resolve(42); console.log(value);";
    let completedRequest: CompletionRequest | undefined;
    h.model.complete = async (request) => {
      completedRequest = request;
      assert.ok(
        request.tools?.some((tool) => tool.function.name === "execute_code"),
      );
      assert.match(
        request.messages[0]!.content,
        /shared across every room and DM/,
      );
      await request.executeTool!(
        "memory",
        JSON.stringify({ action: "set", key: "scripts/answer", value: source }),
      );
      const memory = JSON.parse(
        await request.executeTool!(
          "memory",
          '{"action":"get","key":"scripts/answer"}',
        ),
      );
      const result = JSON.parse(
        await request.executeTool!(
          "execute_code",
          JSON.stringify({ code: memory.value }),
        ),
      );
      assert.equal(result.ok, true, result.error ?? result.stderr);
      assert.equal(result.stdout, "42\n");
      const saved = JSON.parse(
        await request.executeTool!(
          "execute_code",
          JSON.stringify({
            code: `await Deno.writeTextFile("answer.ts", ${JSON.stringify(
              source,
            )});`,
          }),
        ),
      );
      assert.equal(saved.ok, true, saved.error ?? saved.stderr);
      return "Saved the calculation.";
    };
    h.server.addMessage("@helper calculate and save a reusable script");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.ok(completedRequest);
    assert.match(
      await completedRequest.executeTool!(
        "execute_code",
        '{"code":"console.log(1)"}',
      ),
      /no longer active/,
    );
    await h.restart();
    h.model.complete = async (request) => {
      const result = JSON.parse(
        await request.executeTool!("execute_code", '{"file":"answer.ts"}'),
      );
      assert.equal(result.ok, true, result.error ?? result.stderr);
      return `Answer: ${String(result.stdout).trim()}`;
    };
    h.server.addMessage("@helper run that script again");
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(h.server.state.sends.at(-1)?.text, "Answer: 42");
    assert.ok(!h.logs.some((line) => line.includes(source)));
  } finally {
    await h.stop();
  }
});

test("stopping the agent kills and reaps active code without a late reply", async () => {
  const h = await harness();
  try {
    h.model.complete = async (request) => {
      await request.executeTool!(
        "execute_code",
        JSON.stringify({
          code: 'await Deno.writeTextFile("started", "yes"); while(true) {}',
        }),
      );
      return "late reply must be suppressed";
    };
    h.server.addMessage("@helper run a long calculation");
    const pending = h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor("subprocess started", () =>
      existsSync(join(h.dir, "workspace", "started")),
    );
    await h.agent.stop();
    await pending;
    assert.equal(h.server.state.sends.length, 0);
    assert.ok(
      !readdirSync(h.dir).some((name) => name.startsWith(".execution-")),
    );
  } finally {
    await h.stop();
  }
});

test("socket baselines avoid /me and append-only intervals avoid retained rescans", async () => {
  const h = await harness();
  try {
    h.server.addMessage("seed", { mentions: [] });
    await h.agent.handleSnapshot(h.server.snapshot());
    let reads = 0;
    const history = h.api.history.bind(h.api);
    h.api.history = (query, signal) => {
      reads++;
      return history(query, signal);
    };
    h.api.me = () => {
      throw new Error("socket baseline must not fetch /me");
    };
    const revisions = h.store.metrics.revisions;
    h.server.addMessage("ordinary chatter", { mentions: [] });
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      reads,
      1,
      "one pinned append read, independent of retained history size",
    );
    assert.equal(h.store.metrics.revisions, revisions);
    h.server.editMessage(h.server.state.messages[0].id, "edited seed");
    h.server.addMessage("another append", { mentions: [] });
    await h.agent.handleSnapshot(h.server.snapshot());
    assert.equal(
      h.store.metrics.revisions,
      revisions + 1,
      "mixed edits must force reconciliation",
    );
    assert.equal(h.store.metrics.edits, 1);
  } finally {
    await h.stop();
  }
});

test("native catalogues reuse an authenticated policy revision and refresh after it changes", async () => {
  const h = await harness();
  try {
    let reads = 0;
    const commands = h.api.serverCommands.bind(h.api);
    h.api.serverCommands = (id) => {
      reads++;
      return commands(id);
    };
    const baseline = () => ({ ...h.server.snapshot(), policy_revision: 1 });
    await h.agent.handleSnapshot(baseline());
    h.server.addMessage("@helper first");
    await h.agent.handleSnapshot(baseline());
    h.server.addMessage("@helper second");
    await h.agent.handleSnapshot(baseline());
    assert.equal(reads, 1);
    await h.agent.handleSnapshot({
      ...h.server.snapshot(),
      policy_revision: 2,
    });
    h.server.addMessage("@helper third");
    await h.agent.handleSnapshot({
      ...h.server.snapshot(),
      policy_revision: 2,
    });
    assert.equal(reads, 2);
    await h.agent.handleSnapshot({
      ...h.server.snapshot(),
      policy_revision: 2,
      server_instance: "restarted",
    });
    h.server.addMessage("@helper after restart");
    await h.agent.handleSnapshot({
      ...h.server.snapshot(),
      policy_revision: 2,
      server_instance: "restarted",
    });
    assert.equal(reads, 3);
  } finally {
    await h.stop();
  }
});

test("plain model deltas are provisional while final output still uses the durable outbox", async () => {
  const h = await harness();
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const previews: string[] = [];
  h.agent.options.setPreview = (_id, text) => previews.push(text);
  h.model.complete = async (request) => {
    request.onDelta?.("Provisional");
    await held;
    return "Final answer";
  };
  try {
    h.server.addMessage("@helper hello");
    const turn = h.agent.handleSnapshot(h.server.snapshot());
    await h.waitFor("first provisional text", () => previews.length > 0);
    assert.equal(previews[0], "Provisional");
    assert.equal(h.server.state.sends.length, 0);
    release();
    await turn;
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.server.state.sends[0].text, "Final answer");
    assert.equal(h.activities.at(-1), "idle");
  } finally {
    release();
    await h.stop();
  }
});
