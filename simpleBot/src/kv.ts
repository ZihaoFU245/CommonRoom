// Narrow runtime boundary keeps the provider-neutral client type-checkable with
// the repository's TypeScript tools. Persistence itself requires Deno KV.
export type Key = (string | number)[];
export interface Entry<T = unknown> {
  key: Key;
  value: T | null;
  versionstamp: string | null;
}
export interface Atomic {
  check(...entries: { key: Key; versionstamp: string | null }[]): Atomic;
  set(key: Key, value: unknown): Atomic;
  delete(key: Key): Atomic;
  commit(): Promise<{ ok: boolean; versionstamp?: string }>;
}
export interface Kv {
  get<T = unknown>(key: Key): Promise<Entry<T>>;
  set(key: Key, value: unknown): Promise<unknown>;
  delete(key: Key): Promise<void>;
  atomic(): Atomic;
  list<T = unknown>(
    selector: { prefix: Key },
    options?: {
      limit?: number;
      cursor?: string;
    },
  ): AsyncIterable<Entry<T>> & { cursor: string };
  close(): void;
}

export async function openKv(path: string): Promise<Kv> {
  const runtime = (
    globalThis as unknown as {
      Deno?: { openKv?: (path: string) => Promise<Kv> };
    }
  ).Deno;
  if (!runtime?.openKv)
    throw new Error("SimpleBot storage requires Deno with --unstable-kv");
  return runtime.openKv(path);
}
