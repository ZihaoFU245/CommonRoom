import { MAX_JSON_BYTES } from "./protocol.ts";

/** Enforce a byte budget while reading, including chunked responses. */
export async function responseJson(response: Response): Promise<unknown> {
  const declared = Number(response.headers.get("Content-Length"));
  if (declared > MAX_JSON_BYTES) {
    await response.body?.cancel();
    throw new Error("Server response exceeds the size limit.");
  }
  const reader = response.body?.getReader();
  if (!reader) return null;
  const decoder = new TextDecoder();
  const parts: string[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_JSON_BYTES)
        throw new Error("Server response exceeds the size limit.");
      parts.push(decoder.decode(value, { stream: true }));
    }
    parts.push(decoder.decode());
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
  try {
    return JSON.parse(parts.join("")) as unknown;
  } catch {
    return null;
  }
}
