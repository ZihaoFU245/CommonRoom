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
  for (const command of ["/system configs", "/sudo /system configs"]) {
    const value = {
      __proto__: "literal",
      label: "```\n# forged\n<script>bad</script>",
      names: ["alice", "你好"],
    };
    const raw = JSON.stringify(value);
    const nodes = output(
      command,
      `# System configuration\n\n\`\`\`json\n${raw}\n\`\`\``,
    );
    const blocks = nodesWithTag(nodes, "pre");
    assert.equal(blocks.length, 1);
    assert.deepEqual(JSON.parse(text(blocks[0].children)), JSON.parse(raw));
    assert.equal(nodesWithTag(nodes, "script").length, 0);
  }
});
test("system metrics preserve server Markdown tables and percentages", () => {
  const nodes = output(
    "/system metrics",
    "# System metrics\n\n- Hit rate: **84.85%**\n\n| Metric | Value |\n| --- | ---: |\n| hits | 4752 |\n| misses | 848 |",
  );
  assert.equal(nodesWithTag(nodes, "h1").length, 1);
  assert.equal(nodesWithTag(nodes, "table").length, 1);
  assert.equal(nodesWithTag(nodes, "strong").length, 1);
});
test("metrics sections fold into native disclosures while the summary stays visible", () => {
  const report =
    "# System metrics\n\n- Hit rate: **84.86%**\n\n## History and unread caches\n\n| Metric | Value |\n| --- | ---: |\n| hits | 4752 |\n\n## Snapshot cache\n\n| Metric | Value |\n| --- | ---: |\n| hits | 10 |\n\n## Database pools\n\nNo extra profiling.\n\n## Server admission\n\nAvailable workers.";
  const nodes = commandNodes(report, true);
  assert.equal(nodesWithTag(nodes, "details").length, 4);
  assert.deepEqual(
    nodesWithTag(nodes, "summary").map((node) => text(node.children)),
    [
      "History and unread caches",
      "Snapshot cache",
      "Database pools",
      "Server admission",
    ],
  );
  assert.equal(nodes[0].tag, "h1");
  assert.ok(text(nodes[1].children).includes("84.86%"));
  assert.equal(
    nodesWithTag(nodesWithTag(nodes, "details")[0].children, "table").length,
    1,
  );
  assert.equal(nodesWithTag(commandNodes(report), "details").length, 0);
  assert.equal(
    nodesWithTag(commandNodes("## <script>bad()</script>", true), "script")
      .length,
    0,
  );
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
    ["/bot ls #room", "helper\nworker"],
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
      "@global": {
        "media.user_quota": { value: "1073741824", source: "config" },
        "media.max_size": { value: "26214400", source: "@global" },
      },
      "#a": {
        "media.max_size": { value: "26214400", source: "@global" },
        "media.max_age": { value: "604800", source: "config" },
        label: { value: "__reviewer__", source: "#a" },
      },
    },
    alice: {
      "@global": {
        "media.user_quota": { value: "536870912", source: "@global" },
      },
    },
  };
  const nodes = output("/user get @all @all", JSON.stringify(data));
  assert.equal(nodesWithTag(nodes, "pre").length, 0);
  assert.equal(nodesWithTag(nodes, "li").length, 6);
  assert.deepEqual(
    nodesWithTag(nodes, "h3").map((node) => text(node.children)),
    ["bob", "alice"],
  );
  for (const value of ["1g", "25m", "1w", "512m", "__reviewer__"])
    assert.ok(text(nodes).includes(value), value);
  assert.deepEqual(
    nodesWithTag(nodes, "h4").map((node) => text(node.children)),
    ["@global", "#a", "@global"],
  );
  for (const origin of ["inherited", "set here"])
    assert.ok(text(nodes).includes(origin));
  for (const label of ["server default", "role default", "inherited from"])
    assert.equal(text(nodes).includes(label), false);
  const rows = nodesWithTag(nodes, "li").map((node) => text(node.children));
  assert.equal(rows[0], "media.user_quota: 1g");
  assert.equal(rows[1], "media.max_size: 25m");
  assert.equal(rows[2], "media.max_size: 25m — inherited");
  assert.equal(rows[4], "label: __reviewer__ — set here");
  assert.equal(nodesWithTag(nodes, "strong").length, 0);
  const hostile = output(
    "/user get bob @all",
    JSON.stringify({
      bob: {
        "@global": {
          label: {
            value: "```\n# forged\n<script>bad</script>",
            source: "@global",
          },
        },
      },
    }),
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

test("cleanup metadata distinguishes role defaults, disabled values and minimum ages", () => {
  const nodes = output(
    "/user get bob #a",
    JSON.stringify({
      bob: {
        "@global": { "history.clean_min_age": { value: "0", source: "role" } },
        "#a": { "history.clean_min_age": { value: "-1", source: "role" } },
      },
    }),
  );
  for (const phrase of ["0 (disabled)", "-1 (unrestricted)", "inherited"])
    assert.ok(text(nodes).includes(phrase), phrase);
  assert.equal(text(nodes).includes("role default"), false);
  const limited = output(
    "/user get bob #a",
    JSON.stringify({
      bob: {
        "#a": {
          "history.clean_min_age": { value: "604800", source: "@global" },
        },
      },
    }),
  );
  assert.ok(text(limited).includes("1w"));
  assert.ok(text(limited).includes("inherited"));
});

test("bot credential metadata is formatted without exposing Markdown from fields", () => {
  const raw = JSON.stringify([
    {
      id: "credential-id",
      label: "[tracking](https://outside.test)",
      kind: "worker",
      created: 100,
      expires: 200,
      conversations: ["room-id"],
      operations: ["read", "send"],
    },
    {
      id: "issuer-id",
      label: "issuer",
      kind: "issuer",
      created: 100,
      expires: null,
      conversations: null,
      operations: null,
    },
  ]);
  const nodes = output("/bot token ls helper", raw);
  assert.equal(nodesWithTag(nodes, "h2").length, 1);
  assert.equal(nodesWithTag(nodes, "a").length, 0);
  assert.equal(nodesWithTag(nodes, "pre").length, 0);
  assert.ok(text(nodes).includes("worker"));
  assert.ok(text(nodes).includes("No expiry"));
  assert.ok(text(nodes).includes("read, send"));
  assert.ok(
    text(nodes).includes(
      new Intl.DateTimeFormat(undefined, {
        dateStyle: "full",
        timeStyle: "long",
      }).format(new Date(100000)),
    ),
  );
  assert.ok(
    text(nodes).includes(
      new Intl.DateTimeFormat(undefined, {
        dateStyle: "full",
        timeStyle: "long",
      }).format(new Date(200000)),
    ),
  );
});
