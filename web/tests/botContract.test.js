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
