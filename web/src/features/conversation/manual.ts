/** Small plain-text format shared by browser manuals and stdin output. */
export type ManualBlock =
  | {
      kind: "title" | "heading" | "subheading" | "paragraph" | "code";
      text: string;
    }
  | { kind: "list"; items: string[] };

export function manualBlocks(text: string): ManualBlock[] {
  const blocks: ManualBlock[] = [];
  const lines = text.split("\n");
  let index = 0;
  while (index < lines.length) {
    const line = lines[index] ?? "";
    index += 1;
    if (!line.trim()) continue;
    if (line === "```") {
      const code: string[] = [];
      while (index < lines.length && lines[index] !== "```") {
        code.push(lines[index] ?? "");
        index += 1;
      }
      index += 1;
      blocks.push({ kind: "code", text: code.join("\n") });
    } else if (line.startsWith("# ")) {
      blocks.push({ kind: "title", text: line.slice(2) });
    } else if (line.startsWith("## ")) {
      blocks.push({ kind: "heading", text: line.slice(3) });
    } else if (line.startsWith("### ")) {
      blocks.push({ kind: "subheading", text: line.slice(4) });
    } else if (line.startsWith("- ")) {
      const items = [line.slice(2)];
      while (lines[index]?.startsWith("- ")) {
        items.push((lines[index] ?? "").slice(2));
        index += 1;
      }
      blocks.push({ kind: "list", items });
    } else {
      const paragraph = [line];
      while (index < lines.length && lines[index]?.trim()) {
        const next = lines[index] ?? "";
        if (/^(#{1,3} |- |```)/u.test(next)) break;
        paragraph.push(next);
        index += 1;
      }
      blocks.push({ kind: "paragraph", text: paragraph.join(" ") });
    }
  }
  return blocks;
}
