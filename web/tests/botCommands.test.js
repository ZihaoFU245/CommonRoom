import assert from "node:assert/strict";
import test from "node:test";
import { botCommandSuggestions } from "../src/features/conversation/botCommands.ts";
import { cursorSuggestions } from "../src/features/conversation/completion.ts";
import { isBotCommandDirectory, parseFrame } from "../src/api/protocol.ts";
const bots = [
  {
    account_id: "bot-id",
    username: "机器人",
    commands: [
      { name: "/model", usage: "/model", description: "Display settings" },
      {
        name: "/history",
        usage: "/history [before|after] <sequence>",
        description: "Read history",
      },
    ],
  },
];
test("bot commands complete only the addressed bot and retain its mention prefix", () => {
  assert.deepEqual(botCommandSuggestions("@机器人 /mo", bots), [
    {
      label: "@机器人 /model",
      description: "Display settings",
      value: "@机器人 /model ",
    },
  ]);
  assert.equal(botCommandSuggestions("@机器人 ", bots).length, 2);
  for (const text of [
    "/model",
    "@other /mo",
    "hello @机器人 /mo",
    "@机器人2 /mo",
  ])
    assert.deepEqual(botCommandSuggestions(text, bots), []);
  assert.deepEqual(botCommandSuggestions("@机器人 /history before ", bots), [
    {
      label: "@机器人 /history [before|after] <sequence>",
      description: "Read history",
      value: null,
    },
  ]);
});
test("caret completion preserves arguments and works independently of server commands", () => {
  const text = "@机器人 /hi 123";
  const result = cursorSuggestions(text, text.indexOf(" 123"), (prefix) =>
    botCommandSuggestions(prefix, bots),
  );
  assert.equal(result.hints[0].value, "@机器人 /history 123");
});
test("command metadata validates nested shapes and invalidations carry only identity", () => {
  assert.equal(
    isBotCommandDirectory({ conversation_id: "room-id", bots }),
    true,
  );
  for (const bad of [
    null,
    { conversation_id: "room-id", bots: [{ username: "bot", commands: [] }] },
    {
      conversation_id: "room-id",
      bots: [
        {
          ...bots[0],
          commands: [
            { name: "/bad command", usage: "/bad", description: "bad" },
          ],
        },
      ],
    },
    {
      conversation_id: "room-id",
      bots: [
        {
          ...bots[0],
          commands: [{ name: "/model", usage: "/rm user", description: "bad" }],
        },
      ],
    },
  ])
    assert.equal(isBotCommandDirectory(bad), false);
  assert.deepEqual(
    parseFrame('{"kind":"bot_commands","conversation_id":"room-id"}'),
    { kind: "bot_commands", conversation_id: "room-id" },
  );
  assert.equal(
    parseFrame('{"kind":"bot_commands","conversation_id":null}'),
    null,
  );
});

test("subcommands descend through three levels and preserve arguments at the caret", () => {
  const directory = [
    {
      ...bots[0],
      commands: [
        {
          name: "/jobs",
          usage: "/jobs",
          description: "Jobs",
          subcommands: [
            {
              name: "schedule",
              usage: "/jobs schedule",
              description: "Schedule",
              subcommands: [
                {
                  name: "daily",
                  usage: "/jobs schedule daily <hour>",
                  description: "Daily schedule",
                },
              ],
            },
          ],
        },
      ],
    },
  ];
  assert.equal(
    isBotCommandDirectory({ conversation_id: "room", bots: directory }),
    true,
  );
  assert.equal(botCommandSuggestions("@机器人 /j", directory)[0].advance, true);
  assert.equal(
    botCommandSuggestions("@机器人 /jobs ", directory)[0].value,
    "@机器人 /jobs schedule ",
  );
  assert.equal(
    botCommandSuggestions("@机器人 /jobs schedule da", directory)[0].value,
    "@机器人 /jobs schedule daily ",
  );
  assert.equal(
    botCommandSuggestions("@机器人 /jobs schedule daily ", directory)[0].value,
    null,
  );
  assert.deepEqual(
    botCommandSuggestions("@机器人 /jobs unknown ", directory),
    [],
  );
  const text = "@机器人 /jobs schedule da 12";
  assert.equal(
    cursorSuggestions(text, text.indexOf(" 12"), (p) =>
      botCommandSuggestions(p, directory),
    ).hints[0].value,
    "@机器人 /jobs schedule daily 12",
  );
  for (const mutate of [
    (c) => {
      c.subcommands = null;
    },
    (c) => {
      c.subcommands[0].name = "/schedule";
    },
    (c) => {
      c.subcommands[0].usage = "/other schedule";
    },
    (c) => {
      c.subcommands.push(structuredClone(c.subcommands[0]));
    },
    (c) => {
      c.subcommands[0].subcommands[0].subcommands = [
        {
          name: "extra",
          usage: "/jobs schedule daily extra",
          description: "Too deep",
        },
      ];
    },
    (c) => {
      c.description = "line\nbreak";
    },
    (c) => {
      c.description = "字".repeat(200);
    },
  ]) {
    const invalid = structuredClone(directory);
    mutate(invalid[0].commands[0]);
    assert.equal(
      isBotCommandDirectory({ conversation_id: "room", bots: invalid }),
      false,
    );
  }
  const wide = structuredClone(directory);
  wide[0].commands[0].subcommands = Array.from({ length: 63 }, (_, i) => ({
    name: `mode${i}`,
    usage: `/jobs mode${i}`,
    description: "Mode",
  }));
  assert.equal(
    isBotCommandDirectory({ conversation_id: "room", bots: wide }),
    true,
  );
  wide[0].commands.push({
    name: "/extra",
    usage: "/extra",
    description: "Exceeds total node limit",
  });
  assert.equal(
    isBotCommandDirectory({ conversation_id: "room", bots: wide }),
    false,
  );
});
