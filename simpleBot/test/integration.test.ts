// Actual worker, Rust server, browser sessions and native model tools; temporary state only.
import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BotApi } from "../src/api.ts";
import { Store } from "../src/store.ts";
import type { ChatMessage, FunctionTool } from "../src/model.ts";
import {
  isBotCommandDirectory,
  parseFrame,
  type Message,
  type BotActivity,
} from "../../web/src/api/protocol.ts";
import { botCommandSuggestions } from "../../web/src/features/conversation/botCommands.ts";
const root = resolve(import.meta.dirname, "../..");
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
async function until(check: () => boolean | Promise<boolean>, label: string) {
  const deadline = Date.now() + 15000;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`Timed out: ${label}`);
    await pause(20);
  }
}
async function stop(child: ChildProcess | undefined) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await until(
    () => child.exitCode !== null || child.signalCode !== null,
    "process shutdown",
  );
}
interface ModelRequest {
  model: string;
  stream: boolean;
  messages: ChatMessage[];
  tools?: FunctionTool[];
}
interface BrowserClient {
  socket: WebSocket;
  cookie: string;
  messages: Map<string, Message>;
  replies: Map<number, { kind: string; text: string }>;
  serial: number;
  invalidations: number;
  activities: BotActivity[];
}

test(
  "actual worker: bot owner/admin controls, model server commands, grants, private context and restart",
  { skip: process.env.BOT_INTEGRATION !== "1", timeout: 90000 },
  async () => {
    const dir = mkdtempSync(join(tmpdir(), "simplebot-tools-"));
    let server: ChildProcess | undefined, bot: ChildProcess | undefined;
    const clients: BrowserClient[] = [];
    const modelRequests: ModelRequest[] = [];
    const gateway = createServer((request, response) => {
      assert.equal(
        request.headers.authorization,
        undefined,
        "keyless model receives no credential",
      );
      let body = "";
      request.setEncoding("utf8");
      request.on("data", (chunk: string) => {
        body += chunk;
      });
      request.on("end", () => {
        const input = JSON.parse(body) as ModelRequest;
        modelRequests.push(input);
        let last = input.messages.at(-1);
        const tickData = input.messages.find((item) =>
          item.content.startsWith("Tick state (untrusted data"),
        );
        if (tickData) {
          const state = JSON.parse(
            tickData.content.slice(tickData.content.indexOf("\n") + 1),
          ) as {
            wake: string;
            jobs: { status: string; result?: string; observed: boolean }[];
          };
          const completed = state.jobs.filter(
            (job) =>
              !job.observed && !["queued", "running"].includes(job.status),
          );
          if (state.wake === "job_results" && completed.length)
            last = {
              role: "tool",
              content: JSON.stringify({ jobs: completed }),
            };
        }
        let message: Record<string, unknown>;
        if (
          last?.role === "tool" ||
          last?.content.startsWith('{"type":"tool_job_results",')
        ) {
          const envelope = JSON.parse(last.content) as {
            job_id?: string;
            status?: string;
            jobs?: { result: string }[];
            result?: string;
            error?: string;
            stdout?: string;
            messages?: [number, string, string, string][];
          };
          const result = envelope.jobs
            ? (JSON.parse(
                envelope.jobs.at(-1)?.result ?? "{}",
              ) as typeof envelope)
            : envelope;
          message =
            envelope.job_id &&
            ["queued", "running"].includes(envelope.status ?? "")
              ? {
                  content: null,
                  tool_calls: [
                    {
                      id: `wait-${modelRequests.length}`,
                      type: "function",
                      function: {
                        name: "wait_jobs",
                        arguments: JSON.stringify({
                          job_ids: [envelope.job_id],
                        }),
                      },
                    },
                  ],
                }
              : {
                  content: `**${result.error ? "Denied" : "Done"}** ${result.result ?? result.error ?? result.stdout ?? result.messages?.map((row) => row[3]).join("; ")}`,
                };
        } else {
          const text =
            input.messages.filter((message) => message.role === "user").at(-1)
              ?.content ?? "";
          const room = /create room ([a-z0-9-]+)/u.exec(text)?.[1];
          const reaction = /react to message ([A-Za-z0-9._:-]+)/u.exec(
            text,
          )?.[1];
          const historyBefore = /load history before sequence (\d+)/u.exec(
            text,
          )?.[1];
          const saveScript = text.includes("save execution script");
          const runScript = text.includes("rerun execution script");
          const shellCommand = text.includes("run shell smoke command");
          const command = room
            ? `/new room ${room}`
            : reaction
              ? `/react ${reaction} 👍`
              : "/whoami";
          message = text.includes("finish silently")
            ? { content: "" }
            : {
                content: null,
                tool_calls: [
                  {
                    id: `call-${modelRequests.length}`,
                    type: "function",
                    function: {
                      name: shellCommand
                        ? "run_shell"
                        : saveScript || runScript
                          ? "execute_code"
                          : historyBefore
                            ? "read_history"
                            : "run_server_command",
                      arguments: JSON.stringify(
                        shellCommand
                          ? {
                              command:
                                "sleep 0.05; printf shell-integration-ok",
                              waitMs: 1,
                              timeoutMs: 3000,
                            }
                          : saveScript
                            ? {
                                code: `await Deno.writeTextFile("reusable.ts", 'const value: number = 42; console.log("execution-result-" + value);'); console.log("execution-script-saved");`,
                              }
                            : runScript
                              ? { file: "reusable.ts" }
                              : historyBefore
                                ? { before: Number(historyBefore), limit: 2 }
                                : { command },
                      ),
                    },
                  },
                ],
              };
        }
        if (
          last?.role === "user" &&
          last.content.startsWith("New group messages")
        ) {
          const rows = JSON.parse(
            last.content.slice(last.content.indexOf("\n") + 1),
          ) as { id: string; text: string; username: string }[];
          const silent = rows.every((row) =>
            row.text.includes("finish silently"),
          );
          const separate = rows.some((row) => row.text.includes("independent"));
          message = {
            content: null,
            tool_calls: [
              {
                id: `finish-${modelRequests.length}`,
                type: "function",
                function: {
                  name: "finish_turn",
                  arguments: JSON.stringify({
                    messages: silent
                      ? []
                      : separate
                        ? rows.map((row) => ({
                            text: `Answer for ${row.username}`,
                            reply_to: row.id,
                          }))
                        : [{ text: "Let's ship Friday." }],
                  }),
                },
              },
            ],
          };
        }
        // Translate this deterministic fixture's selected effects into one tick decision.
        const selected = message.tool_calls as
          | { id: string; function: { name: string; arguments: string } }[]
          | undefined;
        const finish = selected?.find(
          (call) => call.function.name === "finish_turn",
        );
        const metadata = input.messages.find((item) =>
          item.content.startsWith("Current message metadata"),
        );
        const inputId = metadata
          ? (
              JSON.parse(
                metadata.content.slice(metadata.content.indexOf(": ") + 2),
              ) as { input_id: string }
            ).input_id
          : undefined;
        const plan = finish
          ? (JSON.parse(finish.function.arguments) as {
              messages: { text: string; reply_to?: string }[];
            })
          : undefined;
        const actions = (selected ?? [])
          .filter(
            (call) =>
              !["finish_turn", "wait_jobs"].includes(call.function.name),
          )
          .map((call) => ({
            id: call.id,
            tool: call.function.name,
            arguments: JSON.parse(call.function.arguments) as unknown,
          }));
        const decision = {
          messages: plan
            ? plan.messages.map((item) => ({
                text: item.text,
                ...(item.reply_to ? { replyTo: item.reply_to } : {}),
              }))
            : typeof message.content === "string" && message.content.trim()
              ? [
                  {
                    text: message.content,
                    ...(inputId ? { replyTo: inputId } : {}),
                  },
                ]
              : [],
          actions,
          next: actions.length
            ? {
                kind: "on_jobs",
                jobs: actions.map((action) => action.id),
                mode: "all",
              }
            : { kind: "wait" },
        };
        message = {
          content: null,
          tool_calls: [
            {
              id: `tick-${modelRequests.length}`,
              type: "function",
              function: {
                name: "finish_tick",
                arguments: JSON.stringify(decision),
              },
            },
          ],
        };
        if (input.stream) {
          response.writeHead(200, { "Content-Type": "text/event-stream" });
          const calls = message.tool_calls as
            | {
                id: string;
                type: string;
                function: { name: string; arguments: string };
              }[]
            | undefined;
          const delta = calls
            ? { tool_calls: calls.map((call, index) => ({ index, ...call })) }
            : message;
          response.end(
            `data: ${JSON.stringify({ choices: [{ delta, finish_reason: calls ? "tool_calls" : "stop" }] })}\n\ndata: [DONE]\n\n`,
          );
        } else
          response.end(
            JSON.stringify({
              choices: [
                {
                  message,
                  finish_reason: message.tool_calls ? "tool_calls" : "stop",
                },
              ],
            }),
          );
      });
    });
    try {
      await new Promise<void>((done) => gateway.listen(0, "127.0.0.1", done));
      const address = gateway.address();
      assert.ok(address && typeof address !== "string");
      let logs = "";
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        CHAT_DATA: join(dir, "server"),
        CHAT_BIND: "127.0.0.1:0",
        CHAT_BASE_URL: "/commonroom/",
        RUST_LOG: "chat=info",
      };
      delete env.CHAT_PRODUCTION;
      server = spawn(join(root, "target/debug/chat"), [], {
        cwd: root,
        env,
        stdio: ["pipe", "pipe", "pipe"],
      });
      server.stdout?.on("data", (chunk: Buffer) => {
        logs += chunk.toString();
      });
      server.stderr?.on("data", (chunk: Buffer) => {
        logs += chunk.toString();
      });
      await until(() => logs.includes("Chat server ready"), "server startup");
      const port = logs.match(/address=127\.0\.0\.1:(\d+)/u)?.[1];
      assert.ok(port);
      const base = `http://127.0.0.1:${port}/commonroom/`;
      server.stdin?.write(
        "/new user alice alice-long-password admin\n/new user bob bob-long-password user\n/new room support\n/add alice support\n/add bob support\n/bot new helper support\n/bot token new helper worker\n",
      );
      await until(() => /bot_[a-f0-9]{64}/u.test(logs), "bot credential");
      const token = logs.match(/bot_[a-f0-9]{64}/u)?.[0];
      assert.ok(token);
      const api = new BotApi({ baseUrl: base, token });
      const baseline = await api.me();
      const conversation = baseline.conversations.find(
        (c) => c.view === "support",
      );
      assert.ok(conversation);
      const conversationId = conversation.id;
      async function login(username: string): Promise<BrowserClient> {
        const response = await fetch(new URL("api/login", base), {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Origin: "http://localhost:5173",
          },
          body: JSON.stringify({
            username,
            password: `${username}-long-password`,
          }),
        });
        assert.equal(response.status, 200);
        const cookie = response.headers.get("set-cookie")?.split(";")[0];
        assert.ok(cookie);
        const socket = new WebSocket(base.replace("http:", "ws:") + "ws", {
          headers: { Cookie: cookie, Origin: "http://localhost:5173" },
        } as unknown as string[]);
        const client: BrowserClient = {
          socket,
          cookie,
          messages: new Map(),
          replies: new Map(),
          serial: 0,
          invalidations: 0,
          activities: [],
        };
        clients.push(client);
        socket.onmessage = (event) => {
          const frame = parseFrame(String(event.data));
          assert.ok(frame, "valid browser frame");
          if (frame.kind === "notice" || frame.kind === "error") {
            if (typeof frame.id === "number")
              client.replies.set(frame.id, frame);
          }
          if (frame.kind === "bot_commands") client.invalidations++;
          if (frame.kind === "activity") client.activities.push(frame);
          const messages =
            frame.kind === "conversation"
              ? frame.messages
              : frame.kind === "snapshot"
                ? frame.rooms.flatMap((room) => room.messages)
                : [];
          for (const message of messages)
            client.messages.set(message.id, message);
        };
        await until(() => socket.readyState === 1, "browser session");
        return client;
      }
      const alice = await login("alice"),
        bob = await login("bob");
      async function say(
        client: BrowserClient,
        text: string,
        room: string | null = "support",
      ) {
        const id = ++client.serial;
        client.socket.send(JSON.stringify({ id, room, text }));
        await until(() => client.replies.has(id), "message acknowledgement");
        assert.equal(
          client.replies.get(id)?.kind,
          "notice",
          client.replies.get(id)?.text ?? "Command failed",
        );
      }
      async function directory(client: BrowserClient) {
        const response = await fetch(
          new URL(`api/bot-commands?conversation_id=${conversationId}`, base),
          {
            headers: { Cookie: client.cookie, Origin: "http://localhost:5173" },
          },
        );
        assert.equal(response.status, 200);
        const value: unknown = await response.json();
        assert.ok(isBotCommandDirectory(value));
        return value.bots;
      }
      const configPath = join(dir, "config.json"),
        statePath = join(dir, "agent.kv");
      writeFileSync(
        configPath,
        JSON.stringify({
          "chat.url": base,
          "model.endpoint": `http://127.0.0.1:${address.port}/`,
          "model.name": "first",
          "chat.historyWindow": 6,
        }),
        { mode: 0o600 },
      );
      let botLogs = "";
      async function startBot() {
        botLogs = "";
        bot = spawn(
          process.execPath,
          [
            "run",
            "--unstable-kv",
            "-A",
            join(root, "simpleBot/src/main.ts"),
            "--with-shell",
          ],
          {
            cwd: root,
            env: {
              ...process.env,
              BOT_TOKEN: token,
              BOT_CONFIG: configPath,
              BOT_STORAGE: statePath,
              CHAT_DATA: dir,
            },
            stdio: ["ignore", "pipe", "pipe"],
          },
        );
        bot.stdout?.on("data", (chunk: Buffer) => {
          botLogs += chunk.toString();
        });
        bot.stderr?.on("data", (chunk: Buffer) => {
          botLogs += chunk.toString();
        });
        await until(
          () => botLogs.includes("authenticated as helper"),
          "worker authentication",
        );
      }
      async function ownMessages() {
        return (
          await api.history({ conversation_id: conversationId })
        ).messages.filter(
          (message) => message.author_id === baseline.account.id,
        );
      }
      await startBot();
      await until(
        async () => (await directory(alice)).length === 1,
        "private catalogue registered",
      );
      assert.equal(
        botCommandSuggestions("@helper /mo", await directory(alice))[0]?.value,
        "@helper /model ",
      );
      assert.deepEqual(
        (await directory(bob)).flatMap((bot) =>
          bot.commands.map((command) => command.name),
        ),
        ["/metrics", "/help", "/ping"],
        "ordinary members do not receive private bot controls",
      );
      // Provisional text travels independently of durable messages and catalogue RPCs.
      const endpoint = api.socket();
      const progress = new WebSocket(endpoint.url, {
        headers: endpoint.headers,
      } as unknown as string[]);
      try {
        await until(() => progress.readyState === 1, "progress socket");
        progress.send(
          JSON.stringify({
            kind: "activity",
            conversation_id: conversationId,
            state: "working",
            draft: "provisional answer",
          }),
        );
        await until(
          () =>
            alice.activities.some(
              (item) => item.draft === "provisional answer",
            ),
          "browser provisional progress",
        );
        assert.ok(
          !(
            await api.history({ conversation_id: conversationId })
          ).messages.some((message) => message.text === "provisional answer"),
        );
        progress.send(
          JSON.stringify({
            kind: "activity",
            conversation_id: conversationId,
            state: "idle",
          }),
        );
        await until(
          () => alice.activities.at(-1)?.state === "idle",
          "progress cleared",
        );
      } finally {
        progress.close();
      }
      await say(bob, "@helper /ping");
      await until(
        async () =>
          (await ownMessages()).some((message) => message.text === "Pong."),
        "public ping without a model call",
      );
      await say(bob, "@helper /metrics");
      await until(
        async () =>
          (await ownMessages()).some(
            (message) =>
              message.text.includes("Metrics for this conversation") &&
              message.markdown &&
              message.text.includes(
                "| Stage | Latest | Mean | P95 | Samples |",
              ) &&
              message.text.includes(
                "compact at 24576 tokens; total 32768 tokens",
              ),
          ),
        "public metrics exposes numeric diagnostics without private controls",
      );
      const scoped = await api.issueCredential({
        label: "demo-scoped",
        expires: Math.floor(Date.now() / 1000) + 600,
        conversations: [conversationId],
        operations: ["read", "send", "react"],
      });
      const scopedApi = new BotApi({ baseUrl: base, token: scoped.token });
      assert.equal((await scopedApi.me()).credential.operations?.length, 3);
      await assert.rejects(scopedApi.serverCommands(conversationId));
      await scopedApi.send(
        {
          conversation_id: conversationId,
          text: "Typed scoped API demonstration",
        },
        "scoped-send",
      );
      await say(alice, `/bot token rm helper ${scoped.id}`, null);
      await assert.rejects(scopedApi.me());
      await say(bob, "@helper /model");
      await until(
        async () =>
          (await ownMessages()).some((m) =>
            m.text.includes("Only this bot's owner"),
          ),
        "non-owner denied",
      );
      assert.equal(modelRequests.length, 0);
      await say(alice, "/bot owner helper bob", null);
      await until(
        async () => (await directory(bob)).length === 1,
        "owner catalogue appears",
      );
      assert.deepEqual(
        botCommandSuggestions("@helper /config ", await directory(bob)).map(
          (hint) => hint.value,
        ),
        ["@helper /config get ", "@helper /config set "],
      );
      await say(bob, '@helper /config set model.name "second"');
      await until(
        async () =>
          (await ownMessages()).some((m) =>
            m.text.includes('"model.name": "second"'),
          ),
        "owner config applied",
      );
      await say(
        bob,
        '@helper /config set prompt "Use Markdown, with clear examples"',
      );
      await until(
        () =>
          JSON.parse(readFileSync(configPath, "utf8")).prompt ===
          "Use Markdown, with clear examples",
        "runtime prompt saved",
      );
      const edited = JSON.parse(readFileSync(configPath, "utf8"));
      edited.prompt = "Use Markdown and concise explanations after reload";
      writeFileSync(configPath, JSON.stringify(edited));
      await say(bob, "@helper /reload");
      await until(
        async () =>
          (await ownMessages()).some((m) =>
            m.text.includes("Configuration reloaded"),
          ),
        "file reload acknowledged",
      );
      // Same server through an equivalent base URL forces a real transport rebuild.
      await say(bob, `@helper /config set chat.url "${base}?reload=1"`);
      await until(
        () => botLogs.split("socket connected").length >= 3,
        "transport setting reconnects",
      );
      await until(
        async () => (await directory(bob)).length === 1,
        "catalogue restored after transport change",
      );
      await say(bob, "@helper /whoami");
      await until(
        async () =>
          (await ownMessages()).some((m) => m.text.includes("Owner: bob")),
        "private bot identity",
      );
      assert.equal(
        modelRequests.length,
        0,
        "maintenance never reaches the model",
      );
      await until(
        () =>
          [...alice.messages.values()].some(
            (m) => m.text === "@helper /whoami",
          ),
        "full invocation reaches other members",
      );
      await say(alice, "/grant helper admin", null);
      await until(
        async () =>
          (await api.serverCommands(conversationId)).commands.some(
            (command) => command.name === "/new",
          ),
        "bot admin command catalogue",
      );
      const createdRoom = await api.createConversation({
        kind: "room",
        name: "typed-demo",
      });
      assert.ok(
        (await api.me()).conversations.some(
          (conversation) => conversation.id === createdRoom.conversation_id,
        ),
      );
      const direct = await api.createConversation({
        kind: "direct",
        username: "bob",
      });
      assert.equal(
        (await api.createConversation({ kind: "direct", username: "bob" }))
          .conversation_id,
        direct.conversation_id,
      );
      await say(bob, "@helper create room model-room");
      await until(
        async () =>
          (await api.me()).conversations.some((c) => c.view === "model-room"),
        "model creates room using bot grants",
      );
      await until(
        async () =>
          (await ownMessages()).some(
            (m) =>
              m.text.startsWith("**Done**") && m.text.includes("model-room"),
          ),
        "model reports actual result",
      );
      assert.equal(modelRequests[0]?.model, "second");
      assert.ok(
        modelRequests[0]?.messages.some(
          (message) =>
            message.content.includes('"username":"bob"') &&
            message.content.includes('"author_id":'),
        ),
      );
      assert.ok(
        !modelRequests[0]?.messages[0]?.content.includes(
          "concise explanations after reload",
        ),
      );
      assert.ok(
        modelRequests[0]?.messages.some(
          (message) =>
            message.role === "user" &&
            message.content.includes("concise explanations after reload"),
        ),
      );
      for (const request of modelRequests)
        for (const message of request.messages)
          for (const secret of [
            "@helper /model",
            "/config set",
            "worker.status",
            "model.name",
          ])
            assert.ok(
              !message.content.includes(secret),
              `private administration stays out of model context: ${secret}`,
            );
      const input = [...bob.messages.values()].find(
        (m) => m.text === "@helper create room model-room",
      );
      assert.ok(input);
      const source = await api.messageContext(conversationId, input.id);
      assert.deepEqual(source.author.groups, ["user"]);
      assert.equal(source.author.id, source.bot.owner_id);
      assert.ok(source.author.permissions?.includes("w:message.create"));
      await api.react(conversationId, input.id, "✅", true);
      await api.react(conversationId, input.id, "✅", true);
      await api.react(conversationId, input.id, "✅", false);
      await say(bob, `@helper react to message ${input.id}`);
      await until(
        async () =>
          (
            await api.history({ conversation_id: conversationId })
          ).messages.some(
            (m) => m.id === input.id && m.reactions["👍"]?.includes("helper"),
          ),
        "native reaction",
      );
      await say(alice, "@helper show server identity");
      await until(
        async () =>
          (await ownMessages()).some(
            (m) =>
              m.text.startsWith("**Done**") &&
              m.text.includes("helper") &&
              m.text.includes("admin"),
          ),
        "model uses server whoami",
      );
      const recent = await api.history({
        conversation_id: conversationId,
        limit: 100,
      });
      const target = recent.messages.find(
        (message) => message.text === "Typed scoped API demonstration",
      );
      assert.ok(target);
      await say(
        bob,
        `@helper load history before sequence ${target.sequence + 1}`,
      );
      await until(
        async () =>
          (await ownMessages()).some(
            (message) =>
              message.text.includes("**Done**") &&
              message.text.includes("Typed scoped API demonstration"),
          ),
        "model selects an older history page",
      );
      assert.ok(
        modelRequests.some((request) =>
          request.messages.some(
            (message) =>
              message.content.startsWith("Tick state (untrusted data") &&
              message.content.includes('"tool":"read_history"'),
          ),
        ),
      );
      await say(bob, "@helper run shell smoke command");
      await until(
        async () =>
          (await ownMessages()).some((message) =>
            message.text.includes("shell-integration-ok"),
          ),
        "shell tool through the actual worker",
      );
      await say(bob, "@helper save execution script");
      await until(
        async () =>
          (await ownMessages()).some((message) =>
            message.text.includes("execution-script-saved"),
          ),
        "model saves a reusable workspace script",
      );
      const repliesBeforeSilence = (await ownMessages()).length;
      await say(bob, "@helper finish silently");
      await until(
        async () =>
          Object.values((await Store.inspect(statePath)).tasks).some(
            (task) =>
              task.status === "answered" && task.silentHash !== undefined,
          ),
        "silent turn checkpointed",
      );
      assert.equal(
        (await ownMessages()).length,
        repliesBeforeSilence,
        "silent turn sends no reply",
      );
      const repliesBeforeRestart = (await ownMessages()).length;
      const count = modelRequests.length;
      await stop(bot);
      await until(
        async () => (await directory(alice)).length === 0,
        "disconnect clears catalogue",
      );
      await startBot();
      await until(
        async () => (await directory(alice)).length === 1,
        "restart registers private commands",
      );
      await pause(80);
      assert.equal(
        modelRequests.length,
        count,
        "restart does not replay model actions",
      );
      assert.equal(
        (await ownMessages()).length,
        repliesBeforeRestart,
        "restart sends no duplicate replies",
      );
      assert.equal(
        (await ownMessages()).filter(
          (message) => message.reply?.id === input.id,
        ).length,
        1,
        "one reply for one mention despite tool actions and reconnect",
      );
      await say(bob, "@helper rerun execution script");
      await until(
        async () =>
          (await ownMessages()).some((message) =>
            message.text.includes("execution-result-42"),
          ),
        "model reruns a saved workspace script after restart",
      );
      await say(alice, "/bot owner helper alice", null);
      await until(
        async () =>
          !(await directory(bob)).some((bot) =>
            bot.commands.some((command) => command.name === "/config"),
          ),
        "ownership removal hides controls",
      );
      await say(bob, '@helper /config set model.name "stolen"');
      await until(
        async () =>
          (await ownMessages()).filter((m) =>
            m.text.includes("Only this bot's owner"),
          ).length >= 2,
        "former owner denied",
      );
      await say(alice, "/rm group helper admin", null);
      await until(
        async () =>
          !(await api.serverCommands(conversationId)).commands.some(
            (c) => c.name === "/new",
          ),
        "grant removal updates model catalogue",
      );
      await assert.rejects(
        api.executeCommand(conversationId, "/new room denied", "denied-key"),
        (error: unknown) =>
          error instanceof Error && "status" in error && error.status === 403,
      );
      const defaultMessage = await api.send(
        { conversation_id: conversationId, text: "**default**" },
        "default-markdown",
      );
      assert.equal(defaultMessage.message.markdown, true);
      const plain = await api.send(
        { conversation_id: conversationId, text: "plain", markdown: false },
        "plain-opt-out",
      );
      assert.equal(plain.message.markdown, false);
      assert.ok(alice.invalidations > 0 && bob.invalidations > 0);
      await say(alice, '@helper /config set autonomous "true"');
      await until(
        () => JSON.parse(readFileSync(configPath, "utf8")).autonomous === true,
        "autonomous enabled through owner control",
      );
      const beforeGroup = modelRequests.length;
      const startedGroup = Date.now();
      await Promise.all([
        say(alice, "When should we ship?"),
        say(bob, "Friday works for the same launch."),
      ]);
      await until(
        async () =>
          (await ownMessages()).some(
            (message) => message.text === "Let's ship Friday.",
          ),
        "combined plain autonomous answer",
      );
      assert.ok(Date.now() - startedGroup >= 900);
      const combined = (await ownMessages()).find(
        (message) => message.text === "Let's ship Friday.",
      );
      assert.equal(combined?.reply, null);
      assert.equal(
        modelRequests.length,
        beforeGroup + 1,
        "the burst needs one decision and no extra completion after finish_tick",
      );
      const nextStarted = Date.now();
      await Promise.all([
        say(alice, "independent question one"),
        say(bob, "independent question two"),
      ]);
      await until(
        async () =>
          (await ownMessages()).filter((message) =>
            message.text.startsWith("Answer for"),
          ).length === 2,
        "independent replies in one turn",
      );
      assert.ok(Date.now() - nextStarted >= 800);
      const separate = (await ownMessages()).filter((message) =>
        message.text.startsWith("Answer for"),
      );
      assert.ok(
        separate.every((message) =>
          message.reply?.text.includes("independent question"),
        ),
      );
      const completed = (await ownMessages()).length;
      await say(bob, "finish silently");
      await until(
        async () =>
          Object.values((await Store.inspect(statePath)).tasks).some(
            (task) =>
              task.autonomous &&
              task.status === "answered" &&
              task.decision?.length === 0,
          ),
        "autonomous silence checkpointed",
      );
      assert.equal((await ownMessages()).length, completed);
      const groupCalls = modelRequests.length;
      await stop(bot);
      await startBot();
      await pause(150);
      assert.equal(
        modelRequests.length,
        groupCalls,
        "restart preserves completed group turns",
      );
      assert.equal((await ownMessages()).length, completed);
      await say(alice, '@helper /config set autonomous "false"');
      await until(
        () => JSON.parse(readFileSync(configPath, "utf8")).autonomous === false,
        "autonomous disabled through owner control",
      );
      assert.ok(!botLogs.includes("ERROR"), "no worker errors after restart");
    } finally {
      for (const client of clients) client.socket.close();
      await stop(bot);
      await stop(server);
      await new Promise<void>((done) => gateway.close(() => done()));
      rmSync(dir, { recursive: true, force: true });
    }
  },
);
