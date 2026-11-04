/** Optional browser persistence. Only expected storage failures use defaults. */
function unavailable(error: unknown): boolean {
  return (
    error instanceof DOMException &&
    (error.name === "SecurityError" || error.name === "QuotaExceededError")
  );
}

export function readStorage(
  key: string,
  storage?: Pick<Storage, "getItem">,
): string | null {
  try {
    return (storage ?? globalThis.localStorage)?.getItem(key) ?? null;
  } catch (error) {
    if (!unavailable(error)) throw error;
    return null;
  }
}

export function writeStorage(
  key: string,
  value: string,
  storage?: Pick<Storage, "setItem">,
): boolean {
  try {
    const target = storage ?? globalThis.localStorage;
    if (!target) return false;
    target.setItem(key, value);
    return true;
  } catch (error) {
    if (!unavailable(error)) throw error;
    return false;
  }
}

export function readStorageJson(
  key: string,
  storage?: Pick<Storage, "getItem">,
): unknown {
  const value = readStorage(key, storage);
  if (value === null) return null;
  try {
    return JSON.parse(value) as unknown;
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return null;
  }
}
