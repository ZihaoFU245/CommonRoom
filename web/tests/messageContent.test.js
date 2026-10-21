import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { hasMessageContent } from "../src/features/conversation/messageContent.ts";

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
