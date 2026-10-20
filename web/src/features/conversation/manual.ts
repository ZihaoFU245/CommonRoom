import type { LocalOutput } from "../../api/protocol.ts";
import { commandText } from "./console.ts";

function longestBackticks(text: string): number {
  return (text.match(/`+/gu) ?? []).reduce(
    (longest, run) => Math.max(longest, run.length),
    0,
  );
}
export function inlineCode(text: string): string {
  const delimiter = "`".repeat(longestBackticks(text) + 1);
  const padding = /^[` ]|[` ]$/u.test(text) ? " " : "";
  return `${delimiter}${padding}${text}${padding}${delimiter}`;
}
export function codeBlock(text: string, language = "text"): string {
  const fence = "`".repeat(Math.max(3, longestBackticks(text) + 1));
  return `${fence}${language}\n${text}\n${fence}`;
}
function literal(text: string): string {
  return text
    .replace(/[\\`*_[\]{}()#+.!|<>~-]/gu, "\\$&")
    .replace(/\n/gu, "  \n");
}
function listing(title: string, text: string): string {
  const lines = text.split("\n").filter((line) => line.trim());
  return `## ${literal(title)}\n\n${
    !lines.length || text === "Done." || text === "None."
      ? "None."
      : lines.map((line) => `- ${inlineCode(line)}`).join("\n")
  }`;
}
function metadataValue(key: string, value: unknown): string {
  const raw = String(value);
  if (
    ["media.max_size", "media.user_quota"].includes(key) &&
    /^\d+$/u.test(raw)
  ) {
    const bytes = BigInt(raw);
    for (const [unit, scale] of [
      ["g", 1073741824n],
      ["m", 1048576n],
      ["k", 1024n],
    ] as const)
      if (bytes > 0n && bytes % scale === 0n) return `${bytes / scale}${unit}`;
  }
  if (key === "media.max_age" && /^\d+$/u.test(raw)) {
    const seconds = Number(raw);
    for (const [unit, scale] of [
      ["w", 604800],
      ["d", 86400],
      ["h", 3600],
      ["m", 60],
    ] as const)
      if (seconds > 0 && seconds % scale === 0)
        return `${seconds / scale}${unit}`;
    return `${raw}s`;
  }
  return raw;
}
function metadataList(value: object): string {
  return Object.entries(value)
    .map(([name, values]: [string, unknown]) => {
      const rows =
        values && typeof values === "object" ? Object.entries(values) : [];
      return `### ${inlineCode(name)}\n\n${
        rows.length
          ? rows
              .map(([key, raw]) => {
                const text = metadataValue(key, raw);
                return text.includes("\n")
                  ? `- ${inlineCode(key)}:\n\n${codeBlock(text)}`
                  : `- ${inlineCode(key)}: ${inlineCode(text)}`;
              })
              .join("\n")
          : "- No metadata."
      }`;
    })
    .join("\n\n");
}
function details(title: string, text: string): string {
  return `## ${literal(title)}\n\n${text
    .split("\n")
    .map((line) => {
      const colon = line.indexOf(": ");
      return colon < 0
        ? literal(line)
        : `- **${literal(line.slice(0, colon))}:** ${inlineCode(line.slice(colon + 2))}`;
    })
    .join("\n\n")}`;
}
/** Keep machine-readable wire output intact; format only its local presentation. */
export function outputMarkdown(
  entry: Pick<LocalOutput, "command" | "result" | "error"> &
    Partial<Pick<LocalOutput, "room">>,
): string {
  const result = entry.result ?? "…";
  const parts = commandText(entry.command).split(/\s+/u);
  const command = parts[0];
  const scopeLabel = (scope?: string) => {
    if (scope === "@global") return "globally";
    const room = !scope ? entry.room : scope;
    if (!room || room === "@command") return "globally";
    return room && !room.startsWith("@")
      ? `in #${room.replace(/^#/u, "")}`
      : "in the selected room";
  };
  if (entry.error) {
    if (result.startsWith("Usage: ")) {
      const usage = result.slice(7);
      const name = usage.split(/\s+/u)[0] ?? "";
      return `## Usage\n\n${codeBlock(
        usage
          .split(" | ")
          .map((form) => (form.startsWith("/") ? form : `${name} ${form}`))
          .join("\n"),
      )}`;
    }
    return literal(result);
  }
  if (
    ["/help", "/man", "/permissions", "/grant", "/revoke", "/clean"].includes(
      command ?? "",
    )
  )
    return result;
  // History may contain arbitrary user text, including Markdown and fences.
  if (command === "/history") return `## History\n\n${codeBlock(result)}`;
  try {
    const value: unknown = JSON.parse(result);
    if (value !== null && typeof value === "object") {
      const metadataScope =
        parts[4] ??
        (/^(?:#|@global$)/u.test(parts[3] ?? "") ? parts[3] : undefined);
      const title =
        command === "/configs"
          ? "Configuration"
          : command === "/user" && parts[1] === "get"
            ? `User metadata for ${parts[2] === "@all" ? "all users" : parts[2]} ${scopeLabel(metadataScope)}`
            : "Result";
      if (command === "/user" && parts[1] === "get")
        return `## ${literal(title)}\n\n${metadataList(value)}`;
      return `## ${literal(title)}\n\n${codeBlock(JSON.stringify(value, null, 2), "json")}`;
    }
  } catch {
    // Ordinary acknowledgements and lists are text.
  }
  if ((command === "/user" && parts[1] === "ls") || command === "/members")
    return listing(
      `Users ${scopeLabel(command === "/members" ? parts[1] : parts[2])}`,
      result,
    );
  if (command === "/room" && parts[1] === "ls")
    return listing("Rooms available globally", result);
  if (command === "/whoami") return details("Your account", result);
  if (command === "/bot") {
    if (parts[1] === "info") {
      const [identity = "", ...rows] = result.split("\n");
      return `## Bot details\n\n${literal(identity)}\n\n${details("Access and status", rows.join("\n"))}`;
    }
    if (parts[1] === "ls") {
      const colon = result.indexOf(": ");
      return colon < 0
        ? literal(result)
        : listing(
            result.slice(0, colon),
            result
              .slice(colon + 2)
              .split(", ")
              .join("\n"),
          );
    }
    if (parts[1] === "token" && ["new", "ls"].includes(parts[2] ?? ""))
      return `## Bot credentials\n\n${codeBlock(result)}`;
  }
  if (command === "/ls") {
    const sections = result.split("\n\n");
    return sections
      .map((section) => {
        const [title = "", ...lines] = section.split("\n");
        return lines.length
          ? listing(title.replace(/:$/u, ""), lines.join("\n"))
          : literal(section);
      })
      .join("\n\n");
  }
  return literal(result);
}
