import MarkdownIt from "markdown-it";
import type { MarkdownNode } from "./markdown.ts";

const detector = new MarkdownIt({ html: true, breaks: true });
// Record the original offsets before Markdown-it normalizes inline token text.
detector.inline.State = class extends detector.inline.State {
  override push(type: string, tag: string, nesting: -1 | 0 | 1) {
    const token = super.push(type, tag, nesting);
    if (type === "html_inline" || type === "code_inline")
      token.meta = { offset: this.pos };
    return token;
  }
};

const literalDetector = new MarkdownIt({ html: false });
literalDetector.inline.State = detector.inline.State;

const voidTags = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);
// Quoted attributes may contain '>'. Comments and raw-text elements are consumed
// as units so apparent tags in their contents never split a preview.
const tagPattern =
  /<!--[\s\S]*?(?:-->|$)|<!\[CDATA\[[\s\S]*?(?:\]\]>|$)|<\?[\s\S]*?\?>|<![^>]*>|<\/?[a-z][\w:-]*(?:[^>"']|"[^"]*"|'[^']*')*>/giu;
function fragmentEnd(text: string, start: number, fallbackEnd: number): number {
  const tags = new RegExp(tagPattern);
  tags.lastIndex = start;
  const first = tags.exec(text);
  if (!first || first.index !== start) return fallbackEnd;
  if (/^<!doctype\s+html\b/iu.test(first[0])) {
    const next = /^\s*(<html(?:\s|>))/iu.exec(text.slice(tags.lastIndex));
    if (next) {
      const rootStart = tags.lastIndex + next[0].indexOf("<");
      return fragmentEnd(text, rootStart, text.length);
    }
  }
  const name = /^<([a-z][\w:-]*)/iu.exec(first[0])?.[1]?.toLowerCase();
  if (!name || voidTags.has(name) || /\/\s*>$/u.test(first[0]))
    return tags.lastIndex;
  if (["script", "style", "textarea", "title"].includes(name)) {
    const closing = new RegExp(`</${name}\\s*>`, "giu");
    closing.lastIndex = tags.lastIndex;
    return closing.exec(text) ? closing.lastIndex : text.length;
  }
  let depth = 1;
  let match: RegExpExecArray | null;
  while ((match = tags.exec(text))) {
    const rawName = /^<([a-z][\w:-]*)/iu.exec(match[0])?.[1]?.toLowerCase();
    if (
      rawName === "script" ||
      rawName === "style" ||
      rawName === "textarea" ||
      rawName === "title"
    ) {
      const closing = new RegExp(`</${rawName}\\s*>`, "giu");
      closing.lastIndex = tags.lastIndex;
      const end = closing.exec(text);
      if (!end) return text.length;
      if (rawName === name && depth === 1) return closing.lastIndex;
      tags.lastIndex = closing.lastIndex;
      continue;
    }
    const closingName = /^<\/([a-z][\w:-]*)/iu
      .exec(match[0])?.[1]
      ?.toLowerCase();
    if (closingName === name) depth -= 1;
    else if (rawName === name && !/\/\s*>$/u.test(match[0])) depth += 1;
    if (depth === 0) return tags.lastIndex;
  }
  return fallbackEnd;
}

export interface HtmlFragment {
  marker: string;
  source: string;
}
export interface HtmlFragments {
  markdown: string;
  fragments: HtmlFragment[];
}

export function restoreHtmlFragments(
  nodes: MarkdownNode[],
  fragments: HtmlFragment[],
): MarkdownNode[] {
  if (fragments.length === 0) return nodes;
  return nodes.flatMap((node): MarkdownNode[] => {
    if (typeof node !== "string") {
      const children = restoreHtmlFragments(node.children, fragments);
      const containsHtml = (values: MarkdownNode[]): boolean =>
        values.some(
          (value) =>
            typeof value !== "string" &&
            (value.html !== undefined || containsHtml(value.children)),
        );
      return [
        {
          ...node,
          tag: node.tag === "p" && containsHtml(children) ? "div" : node.tag,
          children,
        },
      ];
    }
    let parts: MarkdownNode[] = [node];
    fragments.forEach((fragment, index) => {
      parts = parts.flatMap((part): MarkdownNode[] => {
        if (typeof part !== "string" || !part.includes(fragment.marker))
          return [part];
        return part
          .split(fragment.marker)
          .flatMap((value, i): MarkdownNode[] =>
            i === 0
              ? [value]
              : [
                  {
                    tag: "div",
                    children: [],
                    html: { source: fragment.source, index },
                  },
                  value,
                ],
          );
      });
    });
    return parts;
  });
}

/** Extract real HTML, keeping code examples and autolinks in the Markdown path. */
export function htmlFragments(input: string): HtmlFragments {
  const text = input.replace(/\r\n?/gu, "\n");
  if (!text.includes("<")) return { markdown: text, fragments: [] };
  let prefix = "COMMONROOMHTMLFRAGMENT";
  while (text.includes(prefix)) prefix += "X";
  const lines = text.split("\n");
  const offsets = [0];
  for (const line of lines)
    offsets.push((offsets.at(-1) ?? 0) + line.length + 1);
  function sourceOffset(
    token: { content: string; map: [number, number] | null },
    offset: number,
  ): number {
    const lineStart = token.map?.[0] ?? 0;
    const before = token.content.slice(0, offset).split("\n");
    const relativeLine = before.length - 1;
    const originalLine = lines[lineStart + relativeLine] ?? "";
    const contentLine = token.content.split("\n")[relativeLine] ?? "";
    const indent = Math.max(0, originalLine.indexOf(contentLine));
    return (
      (offsets[lineStart + relativeLine] ?? 0) +
      indent +
      (before.at(-1)?.length ?? 0)
    );
  }
  const literalRanges: { start: number; end: number }[] = [];
  for (const token of literalDetector.parse(text, {})) {
    if (!token.map) continue;
    if (token.type === "fence" || token.type === "code_block") {
      literalRanges.push({
        start: offsets[token.map[0]] ?? 0,
        end: offsets[token.map[1]] ?? text.length,
      });
    } else if (token.type === "inline") {
      for (const child of token.children ?? []) {
        const offset = child.meta?.offset;
        if (child.type !== "code_inline" || typeof offset !== "number")
          continue;
        const runs = /`+/gu;
        runs.lastIndex = offset + child.markup.length;
        let closing: RegExpExecArray | null;
        while ((closing = runs.exec(token.content))) {
          if (closing[0] === child.markup) {
            literalRanges.push({
              start: sourceOffset(token, offset),
              end: sourceOffset(token, runs.lastIndex),
            });
            break;
          }
        }
      }
    }
  }
  const candidates: { start: number; end: number }[] = [];
  const documentStart = /^\s*(?=<!doctype\s+html\b|<html(?:\s|>))/iu.exec(text);
  if (documentStart)
    candidates.push({ start: documentStart[0].length, end: text.length });
  for (const token of detector.parse(text, {})) {
    if (!token.map) continue;
    const lineStart = token.map[0];
    const base = offsets[lineStart] ?? 0;
    if (token.type === "html_block") {
      const blockEnd = Math.min(
        text.length,
        offsets[token.map[1]] ?? text.length,
      );
      const tags = new RegExp(tagPattern);
      tags.lastIndex = base;
      let match: RegExpExecArray | null;
      while ((match = tags.exec(text)) && match.index < blockEnd)
        candidates.push({ start: match.index, end: tags.lastIndex });
    } else if (token.type === "inline") {
      for (const child of token.children ?? []) {
        if (child.type !== "html_inline") continue;
        const offset = child.meta?.offset;
        if (typeof offset !== "number") continue;
        const start = sourceOffset(token, offset);
        candidates.push({ start, end: start + child.content.length });
      }
    }
  }
  candidates.sort((a, b) => a.start - b.start);
  const fragments: HtmlFragment[] = [];
  let markdown = "";
  let cursor = 0;
  for (const candidate of candidates) {
    if (candidate.start < cursor) continue;
    if (
      (!documentStart || candidate.start !== documentStart[0].length) &&
      literalRanges.some(
        (range) =>
          candidate.start >= range.start && candidate.start < range.end,
      )
    )
      continue;
    const end = fragmentEnd(text, candidate.start, candidate.end);
    const previous = fragments.at(-1);
    const leadingDocument =
      cursor === 0 && documentStart?.[0].length === candidate.start;
    const gap = leadingDocument ? "" : text.slice(cursor, candidate.start);
    if (previous && /^\s*$/u.test(gap)) {
      previous.source += text.slice(cursor, end);
      cursor = end;
      continue;
    }
    const marker = `${prefix}${fragments.length}END`;
    markdown += gap + marker;
    fragments.push({
      marker,
      source: text.slice(leadingDocument ? 0 : candidate.start, end),
    });
    cursor = end;
  }
  return { markdown: markdown + text.slice(cursor), fragments };
}
