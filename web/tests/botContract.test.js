import assert from "node:assert/strict";
import test from "node:test";
import { schema, validate } from "../scripts/bot/contract.mjs";

const message = {
  id: "message-id",
  from: "用户",
  author_id: "author-id",
  text: "Hello 🙂",
  markdown: true,
  edited: false,
  time: 1,
  sequence: 1,
  reply: null,
  mentions: ["helper"],
  reactions: { "👍": ["helper"] },
};
const error = {
  code: "forbidden",
  message: "Permission denied.",
  retryable: false,
};
const snapshot = {
  kind: "snapshot",
  account: { id: "account-id", username: "helper" },
  credential: {
    id: "credential-id",
    expires: null,
    conversations: null,
    operations: null,
  },
  conversations: [
    {
      id: "conversation-id",
      view: "support",
      content_revision: 1,
      permissions: { send: true, reply: true, react: true },
      members: ["helper"],
      unread: { count: 1, first: 1, through: 1, oldest: 1, revision: 1 },
    },
  ],
};

test("Bot HTTP and both WebSocket directions have complete named contracts", () => {
  for (const [name, value] of [
    ["Snapshot", snapshot],
    [
      "MessageRequest",
      { conversation_id: "conversation-id", text: "answer", reply_to: null },
    ],
    ["SendResponse", { message, replayed: false }],
    [
      "HistoryQuery",
      { conversation_id: "conversation-id", after: 0, limit: 100 },
    ],
    [
      "HistoryResponse",
      {
        conversation_id: "conversation-id",
        messages: [message],
        content_revision: 1,
        has_before: false,
        has_after: false,
      },
    ],
    [
      "ReadRequest",
      {
        conversation_id: "conversation-id",
        through: 1,
        acknowledge_shared_unread: true,
      },
    ],
    [
      "ReactionRequest",
      {
        conversation_id: "conversation-id",
        message_id: "message-id",
        reaction: "👍",
        active: true,
      },
    ],
    ["Ok", { ok: true }],
    ["ErrorResponse", { error }],
  ])
    assert.equal(validate(name, value), value);
  for (const frame of [
    snapshot,
    {
      kind: "conversation",
      conversation_id: "conversation-id",
      content_revision: 1,
    },
    {
      kind: "config_request",
      id: "request-id",
      operation: "fetch",
      key: null,
      value: null,
    },
    { kind: "config_ack", id: null },
    { kind: "config_ack", id: "request-id" },
    { kind: "error", id: null, error },
  ])
    validate("ServerFrame", frame);
  for (const frame of [
    { kind: "config_ready" },
    { kind: "config_response", id: "request-id", values: { prompt: "帮助" } },
    { kind: "config_response", id: "request-id", error: "Unsupported key" },
  ])
    validate("ClientFrame", frame);
  assert.equal(schema.$id, "urn:commonroom:bot-api:v1");
});

test("the contract rejects browser payloads, ambiguous control responses and invalid bounds", () => {
  for (const [name, value] of [
    ["MessageRequest", { view: "support", id: 1, text: "answer" }],
    [
      "MessageRequest",
      { conversation_id: "conversation-id", text: "answer", unexpected: true },
    ],
    ["HistoryQuery", { conversation_id: "conversation-id", limit: 101 }],
    [
      "ReadRequest",
      { conversation_id: "conversation-id", through: 9007199254740992 },
    ],
    [
      "ReactionRequest",
      {
        conversation_id: "conversation-id",
        message_id: "message-id",
        reaction: " 👍 ",
        active: true,
      },
    ],
    ["ClientFrame", { id: 1, room: "support", text: "/help" }],
    [
      "ClientFrame",
      { kind: "config_response", id: "request-id", values: {}, error: "both" },
    ],
    ["ClientFrame", { kind: "config_response", id: "request-id" }],
    ["ServerFrame", { kind: "notice", id: 1, text: "browser ack" }],
    [
      "ServerFrame",
      {
        kind: "conversation",
        conversation_id: "conversation-id",
        messages: [],
      },
    ],
  ])
    assert.throws(() => validate(name, value), /Invalid/);
});

test("v1 command discovery has bounded typed request and response contracts", () => {
  validate("ServerFrame", {
    kind: "commands_request",
    id: "request-id",
    conversation_id: "room-id",
  });
  for (const frame of [
    { kind: "commands_ready" },
    { kind: "commands_changed" },
    {
      kind: "commands_response",
      id: "request-id",
      commands: [
        { name: "/model", usage: "/model", description: "Display settings" },
      ],
    },
  ])
    validate("ClientFrame", frame);
  for (const frame of [
    {
      kind: "commands_response",
      id: "request-id",
      commands: new Array(65).fill({
        name: "/a",
        usage: "/a",
        description: "a",
      }),
    },
    {
      kind: "commands_response",
      id: "request-id",
      commands: [{ name: "no-slash", usage: "no-slash", description: "a" }],
    },
    { kind: "commands_ready", account_id: "someone-else" },
    { kind: "commands_response", commands: [] },
  ])
    assert.throws(() => validate("ClientFrame", frame), /Invalid/);
});
test("v1 subcommands use bare names with at most three command levels", () => {
  const command = {
    name: "/history",
    usage: "/history",
    description: "History",
    subcommands: [
      {
        name: "before",
        usage: "/history before",
        description: "Before",
        subcommands: [
          {
            name: "page",
            usage: "/history before page",
            description: "Page",
          },
        ],
      },
    ],
  };
  const frame = {
    kind: "commands_response",
    id: "request-id",
    commands: [command],
  };
  validate("ClientFrame", frame);
  command.subcommands[0].name = "/before";
  assert.throws(() => validate("ClientFrame", frame), /Invalid/);
  command.subcommands[0].name = "before";
  command.subcommands[0].subcommands[0].subcommands = [
    {
      name: "extra",
      usage: "/history before page extra",
      description: "Too deep",
    },
  ];
  assert.throws(() => validate("ClientFrame", frame), /Invalid/);
});

test("native commands, source context and private bot catalogues have distinct v1 contracts", () => {
  validate("ServerCommandsQuery", { conversation_id: "room-id" });
  validate("ServerCommandRequest", {
    conversation_id: "room-id",
    command: "/new room study",
  });
  validate("ServerCommandResponse", { result: "Created room study" });
  validate("MessageContextResponse", {
    author: {
      id: "actor-id",
      username: "alice",
      groups: ["user"],
      permissions: null,
    },
    bot: {
      id: "bot-id",
      groups: ["admin"],
      owner_id: "actor-id",
      owner: "alice",
    },
  });
  validate("ClientFrame", {
    kind: "commands_response",
    id: "query-id",
    commands: [
      {
        name: "/config",
        usage: "/config get [key]",
        description: "Private settings",
        operator_only: true,
        subcommands: [
          {
            name: "get",
            usage: "/config get [key]",
            description: "Read settings",
          },
        ],
      },
    ],
  });
  assert.throws(
    () =>
      validate("ServerCommandRequest", {
        conversation_id: "room-id",
        command: "/whoami",
        actor_id: "forged",
      }),
    /Invalid/,
  );
  assert.throws(
    () =>
      validate("MessageContextQuery", {
        conversation_id: "room-id",
        message_id: "message-id",
        groups: ["admin"],
      }),
    /Invalid/,
  );
});
