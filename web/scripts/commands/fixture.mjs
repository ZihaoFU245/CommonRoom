import { h, render } from "preact";
import { useState } from "preact/hooks";
import { ConsoleOutput } from "../../src/components/ConsoleOutput.tsx";
import "../../src/style.css";

const help = `# Commands

Available in this context. Use \`/man command\` for examples, scopes and required permissions.

## Account

### \`/user\`

List users, inspect or update metadata, and manage accounts

- \`/user ls [scope]\` — List accounts globally or members of a room
- \`/user get who [key] [scope]\` — Read one user's metadata, or use @all for the scope
- \`/user set who key value [scope]\` — Set scoped metadata for one user or @all
- \`/user rename [who] name\` — Rename yourself or an authorized account
- \`/user new name password [user|admin]\` — Create an account; the default group is user

## Rooms

### \`/room\`

List and manage rooms and membership

- \`/room ls\` — List rooms you can discover or read
- \`/room new #name\` — Create an invitation-only room
- \`/room rename #name new-name\` — Rename a room while preserving its identity and history

## Browser

- \`/alias A B\` — Map a command prefix in this browser
- \`/rooms\` — Alias for \`/room ls\``;
const entries = [
  ["/help", help],
  [
    "/man user",
    `# /user

- Manage accounts and scoped metadata.

## Usage

\`\`\`text
/user ls [scope]
/user get who [key] [scope]
/user set who key value [scope]
/user rename [who] new-name
\`\`\`

## Metadata

- \`media.max_size\`: maximum bytes per file.
- \`media.max_age\`: retention duration, or **-1** for permanent.
- Choose \`@global\` or \`#support\`.
- Use \`@all\` for every account in that scope.
- HTML stays text: <script>window.bad = true</script>.`,
  ],
  ["/user ls", "alice\nbob\n测试用户🙂"],
  [
    "/user get @all",
    JSON.stringify({
      alice: {
        "@global": {
          "media.user_quota": { value: "1073741824", source: "config" },
          "media.max_size": { value: "26214400", source: "config" },
        },
        "#support": {
          label: { value: "owner", source: "#support" },
          "media.max_size": { value: "10485760", source: "#support" },
        },
      },
      bob: {
        "@global": {
          "media.user_quota": { value: "1073741824", source: "config" },
          "media.max_size": { value: "26214400", source: "@global" },
        },
        "#support": {
          label: { value: "member", source: "#support" },
          "media.max_size": { value: "26214400", source: "@global" },
        },
      },
    }),
  ],
  [
    "/bot info helper",
    "helper (stable-account-id) at support\nOwner: alice\nDisabled: false\nConnected: 1\nPermissions: r:message.read, w:message.create, w:media.upload\nCommands: /user, /room, /help, /man, /history, /reply",
  ],
  [
    "/history",
    "#42 alice: **literal Markdown**\n```\n# This is message text, not a heading\n```\n#43 bob: 你好🙂",
  ],
  [
    "/room delete #support",
    "Permission required: x:room.delete at support. Check /permissions #support for effective access.",
    true,
  ],
];
function Fixture() {
  const [compact, setCompact] = useState(false);
  const [dark, setDark] = useState(false);
  return h(
    "main",
    {
      style: {
        maxWidth: compact ? "360px" : "960px",
        margin: "24px auto",
        padding: "0 12px",
      },
    },
    h("h1", null, "Command output checks"),
    h("p", null, "Isolated presentation fixture. No accounts or server data."),
    h(
      "label",
      null,
      h("input", {
        type: "checkbox",
        checked: compact,
        onChange: (event) => setCompact(event.currentTarget.checked),
      }),
      " Compact width",
    ),
    " ",
    h(
      "label",
      null,
      h("input", {
        type: "checkbox",
        checked: dark,
        onChange: (event) => {
          const enabled = event.currentTarget.checked;
          setDark(enabled);
          document.documentElement.dataset.theme = enabled ? "dark" : "light";
        },
      }),
      " Dark theme",
    ),
    h(
      "div",
      { class: "console-history" },
      entries.map(([command, result, error = false], index) =>
        h(ConsoleOutput, {
          key: command,
          entry: {
            kind: "command",
            key: `command-${index}`,
            order: index,
            room: "support",
            command,
            result,
            error,
          },
        }),
      ),
    ),
  );
}
render(h(Fixture), document.getElementById("app"));
