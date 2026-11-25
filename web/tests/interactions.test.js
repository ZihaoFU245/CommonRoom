import { test } from "node:test";
import assert from "node:assert/strict";
import {
  mentionSuggestions,
  mentionedText,
  mentionedCommand,
} from "../src/features/conversation/interactions.ts";
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

test("room-wide mentions complete and highlight separately from individual mentions", () => {
  assert.deepEqual(mentionSuggestions("hi @al", ["alice"], true), [
    {
      label: "@all",
      description: "Mention everyone in this room",
      value: "hi @all ",
    },
    { label: "@alice", description: "Mention", value: "hi @alice " },
  ]);
  assert.deepEqual(mentionSuggestions("hi @all", ["alice"]), []);
  assert.equal(
    mentionSuggestions("hi @all", ["all"])[0].description,
    "Mention",
  );
  assert.deepEqual(
    mentionedText("@all! @alice @@all mail@all.test @allies", ["alice"], true)
      .filter((part) => part.mention)
      .map((part) => part.text),
    ["@all", "@alice"],
  );
  assert.equal(mentionedCommand("@all /whoami", []), null);
});
