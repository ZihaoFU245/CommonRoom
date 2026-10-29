import { ApiError } from "../../api/client.ts";
import type { BotCommandDirectory, BotCommands } from "../../api/protocol.ts";

export const BOT_COMMAND_REFRESH_MS = 120_000;
interface Entry {
  members: string;
  bots: BotCommands[];
}
interface Request {
  key: string;
  controller: AbortController;
}

/** One session-local directory request covers every readable conversation. */
export class BotCommandCache {
  private readonly entries = new Map<string, Entry>();
  private request: Request | undefined;
  private completedKey: string | undefined;
  private nextRefresh = 0;
  private signature = "";
  private refreshDelay = BOT_COMMAND_REFRESH_MS;
  private retryAfter = 0;
  private failures = 0;
  private stopped = false;
  private readonly fetch: (signal: AbortSignal) => Promise<BotCommandDirectory>;
  private readonly changed: () => void;
  constructor(
    fetch: (signal: AbortSignal) => Promise<BotCommandDirectory>,
    changed: () => void,
  ) {
    this.fetch = fetch;
    this.changed = changed;
  }
  get(id: string, members: string): BotCommands[] | undefined {
    const entry = this.entries.get(id);
    return entry?.members === members ? entry.bots : undefined;
  }
  refresh(
    conversations: ReadonlyMap<string, string>,
    versions: ReadonlyMap<string, number>,
  ) {
    if (this.stopped) return;
    for (const [id, entry] of this.entries)
      if (conversations.get(id) !== entry.members) this.entries.delete(id);
    const scope = [...conversations].sort(([a], [b]) => a.localeCompare(b));
    const key = JSON.stringify(
      scope.map(([id, members]) => [id, members, versions.get(id) ?? 0]),
    );
    if (
      this.request?.key === key ||
      (this.completedKey === key && Date.now() < this.nextRefresh) ||
      Date.now() < this.retryAfter
    )
      return;
    this.request?.controller.abort();
    const request = { key, controller: new AbortController() };
    this.request = request;
    this.fetch(request.controller.signal)
      .then((result) => {
        if (
          this.stopped ||
          request.controller.signal.aborted ||
          this.request !== request
        )
          return;
        this.entries.clear();
        for (const { conversation_id: id, bots } of result.conversations) {
          const members = conversations.get(id);
          if (members !== undefined) this.entries.set(id, { members, bots });
        }
        const signature = JSON.stringify(result);
        this.refreshDelay =
          this.completedKey === key && this.signature === signature
            ? Math.min(this.refreshDelay * 2, 15 * 60_000)
            : BOT_COMMAND_REFRESH_MS;
        this.signature = signature;
        this.completedKey = key;
        this.nextRefresh = Date.now() + this.refreshDelay;
        this.retryAfter = 0;
        this.failures = 0;
        this.changed();
      })
      .catch((error: unknown) => {
        if (
          this.stopped ||
          request.controller.signal.aborted ||
          this.request !== request
        )
          return;
        this.failures++;
        this.retryAfter =
          Date.now() +
          Math.min(30_000 * 2 ** Math.min(this.failures - 1, 4), 300_000);
        if (
          error instanceof ApiError &&
          [401, 403, 404].includes(error.status)
        ) {
          this.entries.clear();
          this.changed();
        }
      })
      .finally(() => {
        if (this.request === request) this.request = undefined;
      });
  }
  stop() {
    this.stopped = true;
    this.request?.controller.abort();
    this.request = undefined;
    this.entries.clear();
  }
}
