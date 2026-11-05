import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  hasMessageContent,
  singleEmoji,
} from "../src/features/conversation/messageContent.ts";

test("blank-message validation matches the server without stripping real Unicode text", () => {
  const cases = JSON.parse(
    readFileSync(
      new URL("./fixtures/messageContent.json", import.meta.url),
      "utf8",
    ),
  );
  for (const [text, expected] of cases) {
    assert.equal(hasMessageContent(text), expected, JSON.stringify(text));
  }
});

test("single emoji detection accepts complete emoji sequences and ignores surrounding whitespace", () => {
  for (const emoji of ["😀", "👩🏽‍💻", "🇭🇰", "❤️", "1️⃣"]) {
    assert.equal(singleEmoji(emoji), emoji);
    assert.equal(singleEmoji(` ${emoji}\n`), emoji);
  }
  for (const text of [
    "",
    "1",
    "©",
    "😀😀",
    "😀 😀",
    "Hi 😀",
    "**😀**",
    "😀‍😀",
  ])
    assert.equal(singleEmoji(text), undefined, JSON.stringify(text));
});
