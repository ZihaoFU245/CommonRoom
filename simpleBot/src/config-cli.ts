// Local helper. Shell quoting supplies a single value argument; JSON escaping is
// performed by ConfigStore, never by concatenating user input into a document.
import { join } from "node:path";
import { CONFIG_KEYS, ConfigStore, isConfigKey } from "./config.ts";

function run(): void {
  const argv = process.argv.slice(2);
  const dataDir =
    process.env.CHAT_DATA ?? join(import.meta.dirname, "..", "data");
  const path = process.env.BOT_CONFIG ?? join(dataDir, "config.json");
  const [key, value] = argv;
  if (argv.length > 2)
    throw new Error(
      "Pass exactly one value argument; use shell double quotes for spaces.",
    );
  if (key !== undefined && !isConfigKey(key))
    throw new Error(`unknown key ${key}; available: ${CONFIG_KEYS.join(", ")}`);
  const store = ConfigStore.load(path);
  const result =
    key === undefined
      ? store.fetch(null)
      : value === undefined
        ? store.fetch(key)
        : store.apply(key, value);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}
try {
  run();
} catch (error) {
  process.stderr.write(`rejected: ${(error as Error).message}\n`);
  process.exitCode = 1;
}
