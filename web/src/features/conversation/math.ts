import { renderToString } from "katex";

/** Generated math markup is trusted only with KaTeX's untrusted-input mode. */
export function mathMarkup(source: string, display: boolean): string {
  return renderToString(source, {
    displayMode: display,
    throwOnError: false,
    trust: false,
    strict: "ignore",
    maxExpand: 1000,
    maxSize: 20,
    macros: {},
  });
}
