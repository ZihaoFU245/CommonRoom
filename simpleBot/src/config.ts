// Runtime configuration owned by this worker.
//
// The worker owns these values; its `/config` chat command applies an
// owner/admin request locally. Startup can succeed with no configuration,
// and settings can be changed live from chat.
import {
  readFileSync,
  renameSync,
  writeFileSync,
  mkdirSync,
  openSync,
  closeSync,
  fsyncSync,
  unlinkSync,
} from "node:fs";
import { randomUUID } from "node:crypto";
import { dirname } from "node:path";

/** Keys an operator may set or fetch. Unknown keys are rejected. */
export const CONFIG_KEYS = [
  "prompt",
  "autonomous",
  "chat.url",
  "chat.token",
  "model.endpoint",
  "model.token",
  "model.name",
  "model.timeoutMs",
  "model.stream",
  "model.maxTokens",
  "model.contextTokens",
  "model.compactTokens",
  "model.reasoningEffort",
  "chat.respondToAllDms",
  "chat.acknowledgeReads",
  "chat.historyWindow",
  "chat.contextChars",
  "model.maxToolCalls",
  "chat.stateLimit",
  "log.level",
] as const;

export type ConfigKey = (typeof CONFIG_KEYS)[number];

export interface Settings {
  autonomous: boolean;
  prompt: string;
  "chat.url": string;
  "chat.token": string;
  "model.endpoint": string;
  "model.token": string;
  "model.name": string;
  "model.timeoutMs": number;
  "model.stream": boolean;
  "model.maxTokens": number;
  "model.contextTokens": number;
  "model.compactTokens": number;
  "model.reasoningEffort": string;
  "chat.respondToAllDms": boolean;
  "chat.acknowledgeReads": boolean;
  "chat.historyWindow": number;
  "chat.contextChars": number;
  "model.maxToolCalls": number;
  "chat.stateLimit": number;
  "log.level": string;
}

export const DEFAULTS: Settings = {
  autonomous: false,
  // Optional personality and user preferences; the system instructions stay in code.
  prompt: "",
  // CommonRoom base URL. Include the hosting base path when the server is not
  // mounted at the root, and omit the /api/bot/v1 suffix.
  "chat.url": "http://127.0.0.1:3000/",
  // Bearer secret. Prefer the seed token supplied through BOT_TOKEN; never put
  // a real secret in this file unless the file is private.
  "chat.token": "",
  // Empty until an operator configures a provider.
  "model.endpoint": "",
  "model.token": "",
  "model.name": "",
  "model.timeoutMs": 120_000,
  "model.stream": true,
  "model.maxTokens": 4096,
  "model.contextTokens": 32768,
  "model.compactTokens": 24576,
  // Empty leaves the provider's reasoning mode unchanged.
  "model.reasoningEffort": "",
  "chat.respondToAllDms": true,
  // Off by default: /read clears the human-facing unread badge for this account.
  "chat.acknowledgeReads": false,
  "chat.historyWindow": 50,
  "chat.contextChars": 131072,
  "model.maxToolCalls": 32,
  // Upper bound on retained task/outbox records in KV.
  "chat.stateLimit": 200,
  "log.level": "info",
};

const LEVELS = ["debug", "info", "warn", "error", "silent"];
const SECRET_KEYS: ConfigKey[] = ["chat.token", "model.token"];

/** Validate one raw string value into its typed form, or explain why not. */
export function coerce(key: ConfigKey, raw: string): string | number | boolean {
  const text = raw.trim();
  switch (key) {
    case "prompt":
      if (Buffer.byteLength(raw, "utf8") > 4096)
        throw new Error("prompt must be at most 4096 UTF-8 bytes");
      return raw;
    case "model.endpoint":
      // Empty clears the endpoint; anything else must be a URL.
      return text.length === 0 ? "" : coerceUrl(text);
    case "chat.url": {
      if (text.length === 0) throw new Error("chat.url cannot be empty");
      return coerceUrl(text);
    }
    case "chat.token":
      if (text.length > 256) throw new Error("token is too long");
      return text;
    case "model.token":
      if (text.length > 512) throw new Error("token is too long");
      return text;
    case "model.name":
      // Empty clears the model id.
      if (text.length > 128)
        throw new Error("model name must be at most 128 characters");
      return text;
    case "model.timeoutMs": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 1_000 || value > 600_000) {
        throw new Error("timeout must be an integer 1000-600000 ms");
      }
      return value;
    }
    case "model.maxTokens": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 128 || value > 32768) {
        throw new Error("output token limit must be an integer 128-32768");
      }
      return value;
    }
    case "model.contextTokens":
    case "model.compactTokens": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 1024 || value > 1048576)
        throw new Error("context token limit must be an integer 1024-1048576");
      return value;
    }
    case "model.reasoningEffort": {
      const value = text.toLowerCase();
      if (!["", "none", "low", "medium", "high"].includes(value)) {
        throw new Error(
          "reasoning effort must be empty, none, low, medium or high",
        );
      }
      return value;
    }
    case "autonomous":
    case "model.stream":
    case "chat.respondToAllDms":
    case "chat.acknowledgeReads": {
      const value = text.toLowerCase();
      if (["1", "true", "on", "yes"].includes(value)) return true;
      if (["0", "false", "off", "no"].includes(value)) return false;
      throw new Error("expected true or false");
    }
    case "chat.contextChars": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 0 || value > 524288)
        throw new Error(
          "context budget must be an integer 0-524288 characters",
        );
      return value;
    }
    case "model.maxToolCalls": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 1 || value > 128)
        throw new Error("tool call limit must be an integer 1-128");
      return value;
    }
    case "chat.historyWindow": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 0 || value > 200) {
        throw new Error("history window must be an integer 0-200");
      }
      return value;
    }
    case "chat.stateLimit": {
      const value = Number(text);
      if (!Number.isSafeInteger(value) || value < 10 || value > 5_000) {
        throw new Error("state limit must be an integer 10-5000");
      }
      return value;
    }
    case "log.level": {
      const value = text.toLowerCase();
      if (!LEVELS.includes(value))
        throw new Error(`level must be one of ${LEVELS.join(", ")}`);
      return value;
    }
    default:
      throw new Error(`unknown key ${key}`);
  }
}

function coerceUrl(text: string): string {
  if (text.length > 512) throw new Error("URL is too long");
  const url = new URL(text);
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error("URL must use http or https");
  }
  // Keep the trailing slash so new URL("api/...", base) resolves predictably.
  return url.toString();
}

export function isConfigKey(key: string): key is ConfigKey {
  return (CONFIG_KEYS as readonly string[]).includes(key);
}

/** Replace every secret with a stable placeholder. Never leak a token. */
export function redact(key: ConfigKey, value: unknown): string {
  if (!SECRET_KEYS.includes(key)) return String(value);
  const text = String(value);
  if (text.length === 0) return "";
  if (text.length <= 8) return "<set>";
  return `<set:${text.slice(0, 4)}…${text.slice(-2)}>`;
}

export class ConfigStore {
  readonly path: string;
  private settings: Settings;
  /** Environment seeds override the file until explicitly changed at runtime. */
  private readonly overrides = new Map<ConfigKey, string | number | boolean>();
  private readonly listeners = new Set<() => void>();

  /** True when this process created the file on first run. */
  readonly created: boolean;

  private constructor(path: string, settings: Settings, created: boolean) {
    this.path = path;
    this.settings = settings;
    this.created = created;
  }

  /** Load settings from disk. A missing file is created; invalid existing files are never overwritten. */
  static load(path: string): ConfigStore {
    let created = false;
    let settings: Settings;
    try {
      settings = readSettings(path);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      settings = { ...DEFAULTS };
      created = true;
    }
    const store = new ConfigStore(path, settings, created);
    if (created) {
      mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
      const lock = `${path}.lock`;
      const descriptor = openSync(lock, "wx", 0o600);
      try {
        try {
          store.settings = readSettings(path);
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
          store.writeSettings(settings);
        }
      } finally {
        closeSync(descriptor);
        unlinkSync(lock);
      }
    }
    return store;
  }

  /** Validate the whole file before replacing any live setting. */
  reload(): { changed: boolean } {
    const next = readSettings(this.path);
    const changed = CONFIG_KEYS.some((key) => this.settings[key] !== next[key]);
    this.settings = next;
    if (changed) for (const listener of this.listeners) listener();
    return { changed };
  }

  all(): Settings {
    const merged = { ...this.settings };
    for (const [key, value] of this.overrides) merged[key] = value as never;
    return merged;
  }

  get<K extends ConfigKey>(key: K): Settings[K] {
    const override = this.overrides.get(key);
    return (override ?? this.settings[key]) as Settings[K];
  }

  /**
   * Seed a value for this process only. Used for BOT_TOKEN/CHAT_URL so an
   * operator's stored setting is not overwritten by the deployment environment.
   */
  override(key: ConfigKey, value: string): void {
    const validated = coerce(key, value);
    const next = this.all();
    next[key] = validated as never;
    validateLimits(next);
    this.overrides.set(key, validated);
  }

  /** Persist one validated value and notify listeners when it changes. */
  set(key: ConfigKey, value: string | number | boolean): { changed: boolean } {
    const validated = coerce(key, String(value));
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    // Fail clearly on concurrent writers; never share a temporary filename or
    // overwrite another writer's successful changes from a stale memory copy.
    const lock = `${this.path}.lock`;
    let descriptor: number;
    try {
      descriptor = openSync(lock, "wx", 0o600);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST")
        throw new Error(
          "Configuration is busy; retry. A stale .lock file may be removed after stopping config writers.",
        );
      throw error;
    }
    let next: Settings;
    try {
      next = readSettings(this.path);
      next[key] = validated as never;
      this.writeSettings(next);
    } finally {
      closeSync(descriptor);
      unlinkSync(lock);
    }
    const changed =
      CONFIG_KEYS.some((name) => this.settings[name] !== next[name]) ||
      this.overrides.has(key);
    this.settings = next;
    this.overrides.delete(key);
    if (changed) for (const listener of this.listeners) listener();
    return { changed };
  }

  /** Apply one setting and return its redacted value. */
  apply(key: ConfigKey, raw: string): Record<string, string> {
    const value = coerce(key, raw);
    this.set(key, value);
    return { [key]: redact(key, value) };
  }

  /** Read redacted worker settings. */
  fetch(key: string | null): Record<string, string> {
    if (key === null) {
      const out: Record<string, string> = {};
      for (const name of CONFIG_KEYS) out[name] = redact(name, this.get(name));
      return out;
    }
    if (!isConfigKey(key)) throw new Error(`unknown key ${key}`);
    return { [key]: redact(key, this.get(key)) };
  }

  /** Settings are advisory to the transport: a reconnect applies them. */
  onReload(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private writeSettings(settings: Settings): void {
    validateLimits(settings);
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    try {
      const descriptor = openSync(temporary, "wx", 0o600);
      try {
        writeFileSync(descriptor, `${JSON.stringify(settings, null, 2)}\n`);
        fsyncSync(descriptor);
      } finally {
        closeSync(descriptor);
      }
      renameSync(temporary, this.path);
    } finally {
      try {
        unlinkSync(temporary);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
    }
  }
}

function readSettings(path: string): Settings {
  const contents = readFileSync(path, "utf8");
  let raw: unknown;
  try {
    raw = JSON.parse(contents);
  } catch {
    throw new Error(
      "Invalid JSON in configuration file; existing file was preserved",
    );
  }
  if (typeof raw !== "object" || raw === null || Array.isArray(raw))
    throw new Error("Configuration must be a JSON object");
  const settings = { ...DEFAULTS };
  for (const [key, value] of Object.entries(raw)) {
    if (!isConfigKey(key)) throw new Error(`unknown configuration key ${key}`);
    if (!["string", "number", "boolean"].includes(typeof value))
      throw new Error(`Invalid configuration value for ${key}`);
    settings[key] = coerce(key, String(value)) as never;
  }
  validateLimits(settings);
  return settings;
}

function validateLimits(settings: Settings): void {
  if (settings["model.compactTokens"] >= settings["model.contextTokens"])
    throw new Error(
      "model.compactTokens must be less than model.contextTokens",
    );
}

/** Chat values use one JSON string: quotes, newlines and backslashes round-trip. */
export function quotedValue(raw: string): string {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new Error(
      'Value must be a double-quoted string, e.g. /config set prompt "Use Markdown"',
    );
  }
  if (typeof value !== "string")
    throw new Error("Value must be a double-quoted string");
  return value;
}

export function formatConfig(values: Record<string, string>): string {
  return "```json\n" + JSON.stringify(values, null, 2) + "\n```";
}
