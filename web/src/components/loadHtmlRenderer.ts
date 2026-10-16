import rendererUrl from "virtual:markdown-renderer-url";
type Renderer = typeof import("../features/conversation/richMarkdown.ts");
let pending: Promise<Renderer> | undefined;
let attempt = 0;

/** Share successful loads across virtual row remounts; never cache failures. */
export function loadHtmlRenderer(): Promise<Renderer> {
  if (!pending) {
    const url = new URL(rendererUrl, import.meta.url);
    if (attempt > 0) url.searchParams.set("retry", String(attempt));
    attempt += 1;
    const load = import(/* @vite-ignore */ url.href) as Promise<Renderer>;
    pending = load.catch((error: unknown) => {
      pending = undefined;
      throw error;
    });
  }
  return pending;
}
