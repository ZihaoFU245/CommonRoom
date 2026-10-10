import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mentionSuggestions,
  mentionEvents,
  mentionedText,
  messageTokens,
  linkHref,
} from "../src/features/conversation/interactions.ts";
const snapshot = (messages = [], direct = []) => ({
  username: "alice",
  rooms: [{ name: "room", messages }],
  direct,
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
test("answer sources become links while mentions and plain text stay text", () => {
  const answer =
    "答案在这里。\n\n来源：\n- [DeepSeek 文档](https://api-docs.deepseek.com/) 与 http://example.com/a.";
  const tokens = messageTokens(answer, []);
  const links = tokens.filter((token) => token.href);
  assert.deepEqual(
    links.map((token) => token.label),
    ["DeepSeek 文档", "example.com/a"],
    "a link shows its title or host, never a full address",
  );
  assert.deepEqual(
    links.map((token) => token.href),
    ["https://api-docs.deepseek.com/", "http://example.com/a"],
  );
  assert.deepEqual(
    links.map((token) => token.text),
    [
      "[DeepSeek 文档](https://api-docs.deepseek.com/)",
      "http://example.com/a.",
    ],
  );
  assert.equal(
    tokens.map((token) => token.text).join(""),
    answer,
    "rendering must not drop or duplicate characters",
  );
  // A mention inside a link-free message is still a mention.
  const mixed = messageTokens("看 @helper 与 https://example.com/x", [
    "helper",
  ]);
  assert.deepEqual(
    mixed.map((token) => [token.text, token.mention, !!token.href]),
    [
      ["看 ", false, false],
      ["@helper", true, false],
      [" 与 ", false, false],
      ["https://example.com/x", false, true],
    ],
  );
});

test("long link titles and addresses are shortened for display", () => {
  const long = `- [${"标".repeat(80)}](https://example.com/${"p".repeat(200)})`;
  const [link] = messageTokens(long, []).filter((token) => token.href);
  assert.ok(link, "the long link is still a link");
  assert.equal(link.label.length, 49, "the label is elided");
  assert.ok(link.label.endsWith("…"));
  assert.ok(link.href.length > 200, "the address itself stays complete");
  assert.equal(
    messageTokens(long, [])
      .map((token) => token.text)
      .join(""),
    long,
    "the source text is never lost",
  );
  // A labelled link with an unusable address stays plain text.
  assert.equal(
    messageTokens("[标题](javascript:alert(1))", []).some(
      (token) => token.href,
    ),
    false,
  );
});

test("only http and https addresses become links", () => {
  assert.equal(linkHref("https://example.com/a"), "https://example.com/a");
  assert.equal(linkHref("http://example.com/a"), "http://example.com/a");
  for (const unsafe of [
    "javascript:alert(1)",
    "data:text/html,<script>",
    "file:///etc/passwd",
    "ftp://example.com",
    "/relative/path",
    "not a url",
  ]) {
    assert.equal(linkHref(unsafe), null, unsafe);
    assert.equal(
      messageTokens(unsafe, []).every((token) => token.href === null),
      true,
      unsafe,
    );
  }
  // A deceptive scheme inside a longer run of text stays text.
  const tokens = messageTokens("see javascript:alert(1) now", []);
  assert.deepEqual(
    tokens.map((token) => [token.text, token.href]),
    [["see javascript:alert(1) now", null]],
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
    mentionEvents(snapshot([old]), snapshot([old, fresh])).length,
    1,
  );
  assert.equal(
    mentionEvents(
      snapshot([old]),
      snapshot([{ ...old, reactions: { "👍": ["bob"] } }]),
    ).length,
    0,
  );
  assert.equal(
    mentionEvents(snapshot([old]), snapshot([old, { ...fresh, from: "alice" }]))
      .length,
    0,
  );
  assert.equal(
    mentionEvents(snapshot([old]), snapshot([{ ...fresh, time: 1 }])).length,
    0,
  );
  assert.equal(
    mentionEvents(
      { username: "alice", rooms: [], direct: [] },
      snapshot([fresh]),
    ).length,
    0,
  );
  const dm = { ...fresh, to: "alice" };
  assert.equal(
    mentionEvents(snapshot(), snapshot([], [dm]))[0].view,
    "@direct:bob",
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
