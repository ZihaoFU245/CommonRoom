import { ApiError } from "../../api/client.ts";
import type { BotCommands } from "../../api/protocol.ts";

interface Entry {
  members: string;
  version: number;
  bots: BotCommands[];
  bytes: number;
}
interface Request {
  id: string;
  members: string;
  version: number;
  controller: AbortController;
}

/** Session-local metadata: four background reads, 32 conversations, 512 KiB. */
export class BotCommandCache {
  private readonly entries = new Map<string, Entry>();
  private readonly requests = new Map<string, Request>();
  private readonly queue: Request[] = [];
  private running = 0;
  private stopped = false;
  private readonly fetch: (
    id: string,
    signal: AbortSignal,
  ) => Promise<{ conversation_id: string; bots: BotCommands[] }>;
  private readonly changed: () => void;
  constructor(
    fetch: (
      id: string,
      signal: AbortSignal,
    ) => Promise<{ conversation_id: string; bots: BotCommands[] }>,
    changed: () => void,
  ) {
    this.fetch = fetch;
    this.changed = changed;
  }
  get(id: string, members: string): BotCommands[] | undefined {
    const entry = this.entries.get(id);
    if (entry?.members !== members) return undefined;
    this.entries.delete(id);
    this.entries.set(id, entry);
    return entry.bots;
  }
  retain(ids: Set<string>) {
    for (const id of this.entries.keys())
      if (!ids.has(id)) this.entries.delete(id);
    for (const [id, request] of this.requests)
      if (!ids.has(id)) {
        request.controller.abort();
        this.requests.delete(id);
      }
  }
  refresh(
    id: string,
    members: string,
    version: number,
    force = false,
    priority = false,
  ) {
    if (this.stopped) return;
    const entry = this.entries.get(id);
    if (!force && entry?.members === members && entry.version === version)
      return;
    const previous = this.requests.get(id);
    if (previous?.members === members && previous.version === version) {
      const queued = this.queue.indexOf(previous);
      if (priority && queued > 0) {
        this.queue.splice(queued, 1);
        this.queue.unshift(previous);
      }
      return;
    }
    previous?.controller.abort();
    for (let index = this.queue.length - 1; index >= 0; index--) {
      if (this.queue[index]?.controller.signal.aborted)
        this.queue.splice(index, 1);
    }
    const request = { id, members, version, controller: new AbortController() };
    this.requests.set(id, request);
    if (priority) this.queue.unshift(request);
    else this.queue.push(request);
    this.pump();
  }
  stop() {
    this.stopped = true;
    for (const request of this.requests.values()) request.controller.abort();
    this.requests.clear();
    this.queue.length = 0;
    this.entries.clear();
  }
  private pump() {
    while (!this.stopped && this.running < 4 && this.queue.length) {
      const request = this.queue.shift();
      if (!request || request.controller.signal.aborted) continue;
      this.running++;
      this.fetch(request.id, request.controller.signal)
        .then((result) => {
          if (
            this.stopped ||
            request.controller.signal.aborted ||
            this.requests.get(request.id) !== request ||
            result.conversation_id !== request.id
          )
            return;
          this.entries.delete(request.id);
          this.entries.set(request.id, {
            members: request.members,
            version: request.version,
            bots: result.bots,
            bytes: new TextEncoder().encode(JSON.stringify(result)).length,
          });
          let bytes = [...this.entries.values()].reduce(
            (sum, entry) => sum + entry.bytes,
            0,
          );
          while (this.entries.size > 32 || bytes > 512 * 1024) {
            const oldest = this.entries.keys().next().value;
            if (oldest === undefined) break;
            bytes -= this.entries.get(oldest)?.bytes ?? 0;
            this.entries.delete(oldest);
          }
          this.changed();
        })
        .catch((error: unknown) => {
          if (
            this.stopped ||
            request.controller.signal.aborted ||
            this.requests.get(request.id) !== request
          )
            return;
          // Network failures preserve suggestions. Authoritative access denials do not.
          if (
            error instanceof ApiError &&
            [401, 403, 404].includes(error.status)
          ) {
            this.entries.delete(request.id);
            this.changed();
          }
        })
        .finally(() => {
          if (this.requests.get(request.id) === request)
            this.requests.delete(request.id);
          this.running--;
          this.pump();
        });
    }
  }
}
