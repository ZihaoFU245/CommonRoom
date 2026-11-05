import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mentionSuggestions,
  messageNotificationEvents,
  mentionedText,
  mentionedCommand,
} from "../src/features/conversation/interactions.ts";
const snapshot = (messages = [], direct = []) => ({
  username: "alice",
  rooms: [{ name: "room", messages }],
  direct,
  unread: {},
});

test("addressed bot commands retain the full literal command and arguments", () => {
  for (const text of [
    "@SimpleBot /whoami",
    "@SimpleBot /history range 10 20",
    "@SimpleBot /long **title** <sample>",
    "@用户🙂\n/schema",
  ]) {
    const name = text.split(/\s/u)[0].slice(1);
    const invocation = mentionedCommand(text, [name]);
    assert.ok(invocation);
    assert.equal(
      invocation.mention + invocation.separator + invocation.command,
      text,
    );
    assert.ok(invocation.command.startsWith("/"));
  }
  assert.deepEqual(mentionedCommand("@SimpleBot /whoami", ["SimpleBot"]), {
    mention: "@SimpleBot",
    separator: " ",
    command: "/whoami",
  });
  for (const text of [
    "@SimpleBot hello",
    "/whoami",
    "hello @SimpleBot /whoami",
    "mail@SimpleBot /whoami",
  ])
    assert.equal(mentionedCommand(text, ["SimpleBot"]), null);
  assert.equal(mentionedCommand("@SimpleBot /whoami", []), null);
});
test("mentions complete room members and preserve Unicode drafts without matching emails", () => {
  assert.equal(
    mentionSuggestions("你好 @b", ["alice", "bob"])[0].value,
    "你好 @bob ",
  );
  assert.deepEqual(mentionSuggestions("mail@b", ["bob"]), []);
  assert.deepEqual(mentionSuggestions("/tell @b", ["bob"]), []);
  assert.deepEqual(mentionSuggestions("你好@b", ["bob"]), []);
  assert.deepEqual(
    mentionedText("你好 @alice! mail@alice.com", ["alice"])
      .filter((p) => p.mention)
      .map((p) => p.text),
    ["@alice"],
  );
});
test("notifications include only new mentions, not own messages, reactions, joined history or cleanup backfill", () => {
  const old = {
    id: "old",
    from: "bob",
    time: 10,
    text: "@alice",
    mentions: ["alice"],
  };
  const fresh = { ...old, id: "new", time: 11 };
  assert.equal(
    messageNotificationEvents(snapshot([old]), snapshot([old, fresh])).length,
    1,
  );
  assert.equal(
    messageNotificationEvents(
      snapshot([old]),
      snapshot([{ ...old, reactions: { "👍": ["bob"] } }]),
    ).length,
    0,
  );
  assert.equal(
    messageNotificationEvents(
      snapshot([old]),
      snapshot([old, { ...fresh, from: "alice" }]),
    ).length,
    0,
  );
  assert.equal(
    messageNotificationEvents(
      snapshot([old]),
      snapshot([{ ...fresh, time: 1 }]),
    ).length,
    0,
  );
  assert.equal(
    messageNotificationEvents(
      { username: "alice", rooms: [], direct: [] },
      snapshot([fresh]),
    ).length,
    0,
  );
});

test("Unicode usernames complete and highlight Chinese, combining marks and emoji", () => {
  for (const name of ["测试", "Zoë", "e\u0301", "用户🙂", "👩‍💻"]) {
    assert.equal(
      mentionSuggestions(`hi @${name}`, [name])[0].value,
      `hi @${name} `,
    );
    assert.deepEqual(
      mentionedText(`hi @${name}!`, [name])
        .filter((part) => part.mention)
        .map((part) => part.text),
      [`@${name}`],
    );
  }
  assert.deepEqual(mentionSuggestions("hi @测试\u3000", ["测试"]), []);
  assert.deepEqual(mentionSuggestions("🙂@测试", ["测试"]), []);
});

test("private notifications include only incoming messages newer than the conversation watermark", () => {
  const incoming = {
    id: "new",
    from: "bob",
    to: "alice",
    sequence: 8,
    text: "hello",
    mentions: [],
  };
  const previous = {
    ...snapshot(),
    unread: { "@direct:bob": { through: 6 }, "@direct:carol": { through: 10 } },
  };
  const next = snapshot(
    [],
    [
      { ...incoming, id: "older", sequence: 6 },
      { ...incoming, id: "own", from: "alice", to: "bob", sequence: 7 },
      incoming,
    ],
  );
  assert.deepEqual(messageNotificationEvents(previous, next), [
    { message: incoming, view: "@direct:bob" },
  ]);
  assert.deepEqual(messageNotificationEvents(next, next), []);
});
