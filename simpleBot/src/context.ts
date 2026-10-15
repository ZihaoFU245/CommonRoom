// Bounded in-memory context. A restart always starts from retained history.
import { createHash } from "node:crypto";
import { BotApi } from "./api.ts";
import type { HistoryQuery, Message } from "./wire.ts";

type Row = [number, string, string, string, number?, string?];

/** Stable history chunks grow by appending; edits/retention and limits rebase. */
export class AppendHistory {
  private rows: Row[] = [];
  private chunks: string[] = [];
  private hashes = new Map<string, string>();
  private used = 0;
  private settings = "";
  private before = 0;

  async prepare(
    api: BotApi,
    conversation: string,
    before: number,
    window: number,
    budget: number,
    filter: (messages: Message[]) => Message[],
  ): Promise<{
    chunks: string[];
    rows: Row[];
    rebuilt: boolean;
    chars: number;
  }> {
    const settings = `${window}:${budget}`;
    let additions: Message[] | undefined;
    const existing = this.chunks.length > 0;
    if (existing && this.settings === settings && before > this.before) {
      // Revalidate the whole bounded prefix against retained state. Never keep
      // an edited, retracted, filtered or expired message merely for cache reuse.
      const retained: Message[] = [];
      let start = this.rows[0]![0];
      let revision: number | undefined;
      for (;;) {
        const page = await api.history({
          conversation_id: conversation,
          start,
          end: before - 1,
          limit: 100,
          ...(revision === undefined ? {} : { content_revision: revision }),
        });
        revision = page.content_revision;
        retained.push(...page.messages);
        const last = page.messages.at(-1)?.sequence;
        if (!page.has_after || last === undefined || last >= before - 1) break;
        if (retained.length > 4096 || last < start) break;
        start = last + 1;
      }
      const visible = filter(retained);
      const prior = visible.filter(
        (message) => message.sequence <= this.rows.at(-1)![0],
      );
      if (
        retained.length <= 4096 &&
        prior.length === this.rows.length &&
        prior.every(
          (message) => this.hashes.get(message.id) === this.hash(message),
        )
      )
        additions = visible.filter(
          (message) => message.sequence > this.rows.at(-1)![0],
        );
    }
    if (additions !== undefined) {
      const rows = additions.map((message): Row => [
        message.sequence,
        message.id,
        message.from,
        message.text.slice(0, 800),
        0,
        message.author_id,
      ]);
      const chunk = JSON.stringify({ messages: rows });
      if (this.used + (rows.length ? chunk.length : 0) > budget)
        additions = undefined;
      else if (rows.length) {
        this.rows.push(...rows);
        this.chunks.push(chunk);
        this.used += chunk.length;
        for (const message of additions)
          this.hashes.set(message.id, this.hash(message));
      }
    }
    const rebuilt = existing && additions === undefined;
    if (additions === undefined) {
      this.rows = [];
      this.chunks = [];
      this.hashes.clear();
      this.used = 0;
      if (window > 0 && budget > 32) {
        const page = await api.history({
          conversation_id: conversation,
          before,
          limit: Math.min(window, 100),
        });
        for (const message of filter(page.messages).reverse()) {
          const row: Row = [
            message.sequence,
            message.id,
            message.from,
            message.text.slice(0, 800),
            0,
            message.author_id,
          ];
          const rows = [row, ...this.rows];
          const chunk = JSON.stringify({ messages: rows });
          if (chunk.length > budget) break;
          this.rows = rows;
          this.hashes.set(message.id, this.hash(message));
        }
        if (this.rows.length) {
          this.chunks = [JSON.stringify({ messages: this.rows })];
          this.used = this.chunks[0]!.length;
        }
      }
    }
    this.settings = settings;
    this.before = before;
    return {
      chunks: [...this.chunks],
      rows: [...this.rows],
      rebuilt,
      chars: this.used,
    };
  }

  private hash(message: Message): string {
    return createHash("sha256")
      .update(
        JSON.stringify([
          message.sequence,
          message.id,
          message.from,
          message.author_id,
          message.text,
        ]),
      )
      .digest("hex");
  }
}

export class ContextWindow {
  private readonly seen = new Map<string, number>();
  private readonly pages = new Map<string, string>();
  private remaining: number;
  private readonly api: BotApi;
  private readonly conversation: string;
  private readonly filter: (messages: Message[]) => Message[];
  constructor(
    api: BotApi,
    conversation: string,
    budget: number,
    filter: (messages: Message[]) => Message[],
  ) {
    this.api = api;
    this.conversation = conversation;
    this.filter = filter;
    this.remaining = budget;
  }

  has(id: string): boolean {
    return this.seen.has(id);
  }

  /** Deduplicate seeded history against later reference/tool retrievals. */
  seed(rows: Row[], chars: number): void {
    for (const row of rows) this.seen.set(row[1], row[3].length);
    this.remaining = Math.max(0, this.remaining - chars);
  }

  async page(
    query: Omit<HistoryQuery, "conversation_id">,
    pageBudget = this.remaining,
    signal?: AbortSignal,
  ): Promise<string> {
    const key = JSON.stringify(query);
    if (this.pages.has(key))
      return JSON.stringify({
        already_loaded: true,
        remaining_chars: this.remaining,
      });
    if (this.remaining <= 0)
      return JSON.stringify({
        error: "History context budget reached; narrow the request",
        remaining_chars: 0,
      });
    const page = await this.api.history(
      {
        ...query,
        conversation_id: this.conversation,
      },
      signal,
    );
    const messages: Row[] = [];
    let truncated = false;
    let allowance = Math.min(pageBudget, this.remaining);
    const candidates = this.filter(page.messages);
    if (query.message_id)
      candidates.sort(
        (a, b) =>
          Number(b.id === query.message_id) - Number(a.id === query.message_id),
      );
    else if (query.around !== undefined)
      candidates.sort(
        (a, b) =>
          Math.abs(a.sequence - (query.around ?? 0)) -
          Math.abs(b.sequence - (query.around ?? 0)),
      );
    else if (
      query.before !== undefined ||
      (query.after === undefined && query.start === undefined)
    )
      candidates.reverse();
    for (const message of candidates) {
      const offset = this.seen.get(message.id) ?? 0;
      if (offset >= message.text.length) continue;
      const overhead =
        message.id.length + message.from.length + message.author_id.length + 32;
      if (Math.min(allowance, this.remaining) <= overhead) {
        truncated = true;
        break;
      }
      const limit = Math.min(
        800,
        this.remaining - overhead,
        allowance - overhead,
      );
      const text = message.text.slice(offset, offset + limit);
      messages.push([
        message.sequence,
        message.id,
        message.from,
        text,
        offset,
        message.author_id,
      ]);
      const used = text.length + overhead;
      this.remaining -= used;
      allowance -= used;
      this.seen.set(message.id, offset + text.length);
      if (offset + text.length < message.text.length) truncated = true;
    }
    messages.sort((a, b) => a[0] - b[0]);
    const result = JSON.stringify({
      messages,
      first: page.messages[0]?.sequence ?? null,
      last: page.messages.at(-1)?.sequence ?? null,
      has_before: page.has_before,
      has_after: page.has_after,
      truncated,
      remaining_chars: Math.max(0, this.remaining),
    });
    if (!truncated) this.pages.set(key, result);
    return result;
  }
}
