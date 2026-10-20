import test from "node:test";
import assert from "node:assert/strict";
import { commandNodes } from "../src/features/conversation/commandMarkdown.ts";
import {
  outputMarkdown,
  inlineCode,
} from "../src/features/conversation/manual.ts";

function nodesWithTag(nodes, tag) {
  return nodes.flatMap((node) =>
    typeof node === "string"
      ? []
      : [
          ...(node.tag === tag ? [node] : []),
          ...nodesWithTag(node.children, tag),
        ],
  );
}
function text(nodes) {
  return nodes
    .map((node) => (typeof node === "string" ? node : text(node.children)))
    .join("");
}
function output(command, result, error = false) {
  return commandNodes(outputMarkdown({ command, result, error }));
}

test("command Markdown renders headings, nested lists, tables and language-labelled fences", () => {
  const nodes = output(
    "/man user",
    '# Users\n\n## Commands\n\n- **Read** `metadata`\n  - scoped members\n\n1. Choose a scope\n2. Read values\n\n| Key | Value |\n| --- | --- |\n| label | member |\n\n```json\n{"label": "member"}\n```',
  );
  for (const tag of [
    "h1",
    "h2",
    "ul",
    "ol",
    "strong",
    "table",
    "thead",
    "td",
    "pre",
    "code",
  ])
    assert.ok(nodesWithTag(nodes, tag).length, tag);
  assert.equal(
    JSON.parse(text(nodesWithTag(nodes, "pre")[0].children)).label,
    "member",
  );
});
test("command documentation never activates HTML, images or unsafe links", () => {
  const source =
    "<script>window.bad = true</script>\n\n![tracking](https://outside.test/pixel)\n\n[bad](javascript:alert(1))\n\n[safe](https://example.com)";
  const nodes = output("/man user", source);
  for (const tag of ["script", "img", "iframe"])
    assert.equal(nodesWithTag(nodes, tag).length, 0);
  assert.ok(text(nodes).includes("<script>"));
  assert.deepEqual(
    nodesWithTag(nodes, "a").map((node) => node.href),
    ["https://outside.test/pixel", "https://example.com"],
  );
});
test("JSON configuration remains exact data inside code blocks", () => {
  for (const command of ["/configs"]) {
    const value = {
      __proto__: "literal",
      label: "```\n# forged\n<script>bad</script>",
      names: ["alice", "你好"],
    };
    const raw = JSON.stringify(value);
    const nodes = output(command, raw);
    const blocks = nodesWithTag(nodes, "pre");
    assert.equal(blocks.length, 1);
    assert.deepEqual(JSON.parse(text(blocks[0].children)), JSON.parse(raw));
    assert.equal(nodesWithTag(nodes, "script").length, 0);
  }
});
test("history and newly issued credentials cannot escape their literal code block", () => {
  const raw =
    "before\n```\n# forged heading\n`````\n![tracking](https://outside.test)\nafter";
  for (const command of [
    "/history",
    "/sudo /history #room",
    "/bot token new helper worker",
    "/bot token ls helper",
  ]) {
    const nodes = output(command, raw);
    assert.equal(nodesWithTag(nodes, "pre").length, 1);
    assert.equal(text(nodesWithTag(nodes, "pre")[0].children).trimEnd(), raw);
    assert.equal(nodesWithTag(nodes, "h1").length, 0);
    assert.equal(nodesWithTag(nodes, "a").length, 0);
  }
});
test("every list and bot detail output has readable Markdown structure", () => {
  for (const command of [
    "/user ls",
    "/room ls",
    "/sudo /room ls",
    "/members",
  ]) {
    const nodes = output(command, "__proto__\n#support");
    assert.equal(nodesWithTag(nodes, "h2").length, 1);
    assert.equal(nodesWithTag(nodes, "li").length, 2);
    assert.ok(text(nodes).includes("__proto__"));
    assert.equal(nodesWithTag(nodes, "strong").length, 0);
  }
  for (const [command, result] of [
    ["/whoami", "Name: __proto__\nPermission: admin"],
    [
      "/bot info helper",
      "helper (id) at room\nOwner: alice\nConnected: 1\nCommands: /user, /room",
    ],
    ["/bot ls #room", "Bot connections at room: helper, worker"],
    [
      "/ls",
      "Your rooms:\n#support\n\nPrivate conversations:\n@private:alice:bob",
    ],
  ])
    assert.ok(nodesWithTag(output(command, result), "li").length, command);
});
test("acknowledgements and errors show literal user input and alternative usage is split", () => {
  const raw =
    "Unknown permission: __proto__ <script>bad()</script> [link](https://outside.test)";
  for (const error of [false, true]) {
    const nodes = output("/user set bob label value", raw, error);
    assert.equal(text(nodes), raw);
    assert.equal(nodesWithTag(nodes, "a").length, 0);
    assert.equal(nodesWithTag(nodes, "strong").length, 0);
  }
  const nodes = output(
    "/bot",
    "Usage: /bot new name [user|admin] | ls [scope] | token ls name",
    true,
  );
  assert.equal(
    text(nodesWithTag(nodes, "pre")[0].children),
    "/bot new name [user|admin]\n/bot ls [scope]\n/bot token ls name\n",
  );
  assert.equal(
    text(commandNodes(inlineCode("/alias ` odd target"))),
    "/alias ` odd target",
  );
});

test("list and metadata headings identify the requested scope and original room", () => {
  for (const [command, room, expected] of [
    ["/user ls #a", "b", "Users in #a"],
    ["/user ls", "a", "Users in #a"],
    ["/sudo /user ls", "a", "Users in #a"],
    ["/user ls @global", "a", "Users globally"],
    ["/user ls", "@command", "Users globally"],
    ["/user get bob", "@command", "User metadata for bob globally"],
    ["/members", "a", "Users in #a"],
    ["/room ls", "a", "Rooms available globally"],
    ["/user get @all #a", "b", "User metadata for all users in #a"],
    ["/user get bob media.max_size #a", "a", "User metadata for bob in #a"],
    ["/user get bob", "a", "User metadata for bob in #a"],
  ]) {
    const nodes = commandNodes(
      outputMarkdown({
        command,
        room,
        result: command.includes("get") ? "{}" : "bob",
      }),
    );
    assert.equal(
      text(nodesWithTag(nodes, "h2")[0].children),
      expected,
      command,
    );
  }
});

test("browser manuals keep explanatory text in bullet points", async () => {
  const { aliasManual } =
    await import("../src/features/conversation/aliases.ts");
  const { markdownManual } =
    await import("../src/features/preferences/markdown.ts");
  for (const manual of [aliasManual, markdownManual]) {
    let fence = false;
    for (const line of manual.split("\n")) {
      if (line.startsWith("```")) {
        fence = !fence;
        continue;
      }
      assert.ok(
        fence || !line || line.startsWith("#") || line.startsWith("- "),
        line,
      );
    }
    assert.equal(fence, false);
  }
});

test("metadata renders as per-user lists with readable effective limits", () => {
  const data = {
    bob: {
      "media.user_quota": "1073741824",
      "media.max_size": "26214400",
      "media.max_age": "604800",
      label: "__reviewer__",
    },
    alice: { "media.user_quota": "536870912" },
  };
  const nodes = output("/user get @all @all", JSON.stringify(data));
  assert.equal(nodesWithTag(nodes, "pre").length, 0);
  assert.equal(nodesWithTag(nodes, "li").length, 5);
  assert.deepEqual(
    nodesWithTag(nodes, "h3").map((node) => text(node.children)),
    ["bob", "alice"],
  );
  for (const value of ["1g", "25m", "1w", "512m", "__reviewer__"])
    assert.ok(text(nodes).includes(value), value);
  assert.equal(nodesWithTag(nodes, "strong").length, 0);
  const hostile = output(
    "/user get bob @all",
    JSON.stringify({ bob: { label: "```\n# forged\n<script>bad</script>" } }),
  );
  assert.equal(nodesWithTag(hostile, "script").length, 0);
  assert.equal(nodesWithTag(hostile, "h1").length, 0);
  assert.equal(nodesWithTag(hostile, "pre").length, 1);
});

test("cleanup preview and execution render their Markdown reports as lists", () => {
  for (const command of [
    "/clean 7d #support --dry-run",
    "/clean 7d @direct:bob",
  ]) {
    const nodes = output(
      command,
      "## Cleanup report\n\n- Scope: `#support`\n- Messages matching: 3\n- Attachment references affected: 1",
    );
    assert.equal(nodesWithTag(nodes, "h2").length, 1);
    assert.equal(nodesWithTag(nodes, "li").length, 3);
    assert.equal(nodesWithTag(nodes, "pre").length, 0);
  }
});
