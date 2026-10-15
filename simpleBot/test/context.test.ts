import assert from "node:assert/strict";
import { test } from "node:test";
import { BotApi } from "../src/api.ts";
import { AppendHistory, ContextWindow } from "../src/context.ts";
import { MockServer } from "./mock-server.ts";

test("history context has a strict text budget, reserves space for retrieval and avoids repeated pages", async () => {
  const server = new MockServer();
  const url = await server.listen();
  try {
    for (let i = 0; i < 20; i++)
      server.addMessage("x".repeat(2000), { mentions: [] });
    const context = new ContextWindow(
      new BotApi({ baseUrl: url, token: server.token }),
      server.conversationId,
      1200,
      (messages) => messages,
    );
    const first = JSON.parse(await context.page({ limit: 2 }, 600));
    assert.ok(first.remaining_chars >= 600);
    assert.equal(first.truncated, true);
    const next = JSON.parse(
      await context.page({
        before: server.state.messages.at(-2)!.sequence,
        limit: 2,
      }),
    );
    assert.ok(next.messages.length > 0);
    assert.ok(next.remaining_chars >= 0);
    assert.ok(
      first.messages.reduce(
        (sum: number, row: unknown[]) => sum + String(row[3]).length,
        0,
      ) +
        next.messages.reduce(
          (sum: number, row: unknown[]) => sum + String(row[3]).length,
          0,
        ) <=
        1200,
    );
    const exhausted = JSON.parse(await context.page({ limit: 1 }));
    assert.ok(exhausted.error || exhausted.truncated);
  } finally {
    await server.close();
  }
});

test("append history keeps a stable prefix beyond 200 messages and rebuilds on edits, retention and character limits", async () => {
  const server = new MockServer();
  const url = await server.listen();
  try {
    const api = new BotApi({ baseUrl: url, token: server.token });
    const session = new AppendHistory();
    for (let i = 0; i < 50; i++)
      server.addMessage(`item ${i}`, { mentions: [] });
    let sequence = server.state.messages.at(-1)!.sequence + 1;
    const prepare = (budget = 131072) =>
      session.prepare(
        api,
        server.conversationId,
        sequence,
        50,
        budget,
        (messages) => messages,
      );
    const first = await prepare();
    for (let i = 50; i < 220; i++)
      server.addMessage(`item ${i}`, { mentions: [] });
    sequence = server.state.messages.at(-1)!.sequence + 1;
    const appended = await prepare();
    assert.equal(appended.rows.length, 220);
    assert.equal(appended.chunks[0], first.chunks[0]);
    assert.equal(appended.rebuilt, false);
    server.editMessage(server.state.messages[0]!.id, "edited history");
    server.addMessage("new item", { mentions: [] });
    sequence = server.state.messages.at(-1)!.sequence + 1;
    const edited = await prepare();
    assert.equal(edited.rebuilt, true);
    assert.equal(edited.rows.length, 50);
    // Removing a retained cached row also invalidates the stable prefix.
    const removed = edited.rows[0]![1];
    server.state.messages = server.state.messages.filter(
      (message) => message.id !== removed,
    );
    server.state.revision++;
    server.addMessage("after removal", { mentions: [] });
    sequence = server.state.messages.at(-1)!.sequence + 1;
    assert.equal((await prepare()).rebuilt, true);
    const compacted = await prepare(256);
    assert.equal(compacted.rebuilt, true);
    assert.ok(compacted.rows.length < 50);
    assert.ok(compacted.chars <= 131072);
  } finally {
    await server.close();
  }
});
