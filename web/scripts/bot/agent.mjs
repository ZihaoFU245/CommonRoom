import { mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { AgentClient, BotApi } from "./client.mjs";

const token = process.env.BOT_TOKEN;
if (!token) throw new Error("Set BOT_TOKEN to a bot credential.");
const statePath =
  process.env.BOT_STATE ||
  join(process.env.CHAT_DATA || "data", "bot-agent-state.json");
await mkdir(dirname(statePath), { recursive: true, mode: 0o700 });
const controller = new AbortController();
process.once("SIGINT", () => controller.abort());
process.once("SIGTERM", () => controller.abort());
const client = new AgentClient({
  api: new BotApi({
    url: process.env.CHAT_URL || "http://127.0.0.1:3000/",
    token,
  }),
  statePath,
  onGap: ({ view }) =>
    console.warn(`History in ${view} may have expired while offline.`),
  onMessage: async (message, { client, username, conversation }) => {
    if (
      !client.snapshot.conversations.find((c) => c.id === conversation)
        ?.permissions.reply
    )
      return;
    if (!message.mentions.includes(username)) return;
    // Replace this fixed answer with your model/tool invocation. Keep the send
    // slot stable across recovery, and make external tool effects retry-safe.
    await client.send(
      "I received your mention. Connect your agent logic here.",
      {
        replyTo: message.id,
        slot: "mention-reply",
      },
    );
  },
});
try {
  await client.run({ signal: controller.signal });
} catch (error) {
  if (!controller.signal.aborted) {
    // Error messages may contain private content from future integrations.
    console.error(
      `Agent stopped (${error.code || error.name}); inspect its checkpoint.`,
    );
    process.exitCode = 1;
  }
}
