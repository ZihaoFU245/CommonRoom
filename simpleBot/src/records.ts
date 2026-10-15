import { mkdir, open, chmod } from "node:fs/promises";
import { dirname } from "node:path";
import { randomUUID } from "node:crypto";
import { openKv, type Kv } from "./kv.ts";

interface Head {
  json?: string;
  generation?: string;
  chunks?: number;
}
export interface Change {
  field: string;
  id: string;
  json?: string;
}

/** Immutable chunks are staged before atomically publishing record heads.
 * A crash exposes either the old or the new transition, including large plans.
 * One worker owns this database; a revision check rejects stale writers. */
export class Records {
  readonly kv: Kv;
  private lease: string | null;
  private constructor(kv: Kv, lease: string | null) {
    this.kv = kv;
    this.lease = lease;
  }

  static async open(path: string): Promise<Records> {
    if (path !== ":memory:") {
      await mkdir(dirname(path), { recursive: true, mode: 0o700 });
      const file = await open(path, "a", 0o600);
      await file.close();
      await chmod(path, 0o600);
    }
    const kv = await openKv(path);
    const records = new Records(kv, null);
    return records;
  }

  async *entries(): AsyncGenerator<[string, string, unknown]> {
    const revision = (await this.kv.get(["worker-revision"])).versionstamp;
    for await (const entry of this.kv.list<Head>({ prefix: ["worker"] })) {
      const [, field, id] = entry.key;
      if (typeof field !== "string" || typeof id !== "string" || !entry.value)
        throw new Error("Invalid worker record key");
      yield [field, id, await this.read(entry.value)];
    }
    // Each opener pins the current commit revision. Another writer causes a
    // conflict instead of silently overwriting state loaded before its commit.
    this.lease = (await this.kv.get(["worker-revision"])).versionstamp;
    if (this.lease !== revision)
      throw new Error("Worker database changed while reading");
  }

  private async read(head: Head): Promise<unknown> {
    if (typeof head.json === "string") return JSON.parse(head.json);
    if (
      typeof head.generation !== "string" ||
      !Number.isSafeInteger(head.chunks) ||
      typeof head.chunks !== "number" ||
      head.chunks < 1 ||
      head.chunks > 1024
    )
      throw new Error("Invalid worker record head");
    const chunks: Uint8Array[] = [];
    for (let i = 0; i < head.chunks; i++) {
      const entry = await this.kv.get<Uint8Array>(["blob", head.generation, i]);
      if (!(entry.value instanceof Uint8Array))
        throw new Error("Missing worker record chunk");
      chunks.push(entry.value);
    }
    const bytes = new Uint8Array(
      chunks.reduce((size, chunk) => size + chunk.length, 0),
    );
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    return JSON.parse(new TextDecoder().decode(bytes));
  }

  async commit(changes: Change[]): Promise<void> {
    if (!changes.length) return;
    // Deletions, cancellations and new output must publish together. Refuse an
    // oversized transition rather than exposing partially committed work.
    if (changes.length > 800)
      throw new Error("Worker transition exceeds 800 records");
    await this.publish(changes);
  }

  private async publish(changes: Change[]): Promise<void> {
    const oldHeads: Head[] = [];
    const transaction = this.kv
      .atomic()
      .check({ key: ["worker-revision"], versionstamp: this.lease });
    const staged: Head[] = [];
    let inlineBytes = 0;
    let publicationAttempted = false;
    try {
      for (const { field, id, json } of changes) {
        const key = ["worker", field, id];
        const old = await this.kv.get<Head>(key);
        if (old.value) oldHeads.push(old.value);
        if (json === undefined) {
          transaction.delete(key);
          continue;
        }
        const bytes = new TextEncoder().encode(json);
        // Small, frequent changes are one direct KV mutation. Larger values
        // use staged chunks, keeping the publication transaction well bounded.
        if (bytes.length <= 16000 && inlineBytes + bytes.length <= 300000) {
          transaction.set(key, { json });
          inlineBytes += bytes.length;
          continue;
        }
        const head = {
          generation: `${Date.now()}:${randomUUID()}`,
          chunks: Math.max(1, Math.ceil(bytes.length / 48000)),
        };
        staged.push(head);
        for (let i = 0; i < head.chunks; i++)
          await this.kv.set(
            ["blob", head.generation, i],
            bytes.slice(i * 48000, (i + 1) * 48000),
          );
        transaction.set(key, head);
      }
      transaction.set(["worker-revision"], randomUUID());
      publicationAttempted = true;
      const result = await transaction.commit();
      if (!result.ok || !result.versionstamp)
        throw new Error(
          "Worker database changed; run only one worker per database",
        );
      this.lease = result.versionstamp;
    } catch (error) {
      // A thrown commit may have an uncertain outcome. Keep staged chunks so
      // a committed head can never point to data removed by error handling.
      if (!publicationAttempted) await this.collect(staged);
      throw error;
    }
    // Garbage collection cannot turn a successful publication into a failed
    // delivery checkpoint. Interrupted cleanup leaves only unreachable chunks.
    await this.collect(oldHeads);
  }

  private async collect(heads: Head[]): Promise<void> {
    for (const head of heads)
      for (let i = 0; i < (head.chunks ?? 0); i++) {
        try {
          await this.kv.delete(["blob", head.generation ?? "", i]);
        } catch {
          /* Reclaim on maintenance. */
        }
      }
  }

  /** Reclaim interrupted large writes after a day, leaving active staging alone. */
  async maintain(): Promise<void> {
    const live = new Set<string>();
    for await (const entry of this.kv.list<Head>({ prefix: ["worker"] }))
      if (entry.value?.generation) live.add(entry.value.generation);
    for await (const entry of this.kv.list({ prefix: ["blob"] })) {
      const generation = entry.key[1];
      if (typeof generation !== "string" || live.has(generation)) continue;
      const created = Number(generation.split(":")[0]);
      if (Number.isFinite(created) && created < Date.now() - 86400000)
        await this.kv.delete(entry.key);
    }
  }

  close(): void {
    this.kv.close();
  }
}
