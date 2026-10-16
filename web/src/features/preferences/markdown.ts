import type { Command } from "../../api/protocol.ts";

export const markdownCommand: Command = {
  name: "/markdown",
  usage: "/markdown on|off",
  description: "Format future messages with Markdown in this browser",
  requirements: "",
  section: "General",
  admin: false,
  console: false,
};
export const markdownManual = `# /markdown

Use /markdown on to format future room messages, private messages and replies.
Use /markdown off to send plain text again. The default is off.
Settings → Preferences → Enable Markdown controls the same sending preference.
Enable LaTeX math renders $...$, $$...$$, \\(...\\) and \\[...\\] in Markdown messages.
Math rendering defaults off and does not require HTML or scripts.

Supports bold, italic, strikethrough, headings, lists, quotes, code and links.
Markdown images and GIFs use ![alt](url) and load without a separate media opt-in.
Remote images disclose your IP to their host; lazy loading and no-referrer limit exposure.
Only absolute HTTP/HTTPS and safe base64 raster image URLs are supported.
Raw HTML displays as text when both Security options are off.
Allow HTML (secure HTML) renders sanitized formatting directly, without a frame.
HTML (code) offers Show text / Render HTML for full HTML, CSS and JavaScript.
Code runs only after selection in an isolated markdown-frame.html preview.
Passive HTML uses no frame; with only HTML (code) enabled it also requires selection.
Editing a message or changing Security options resets the per-message choice.
Full HTML documents preserve indentation. Fences and inline code stay literal.
Frames allow inline code and data media, blocking external dependencies, chat DOM,
chat storage, forms, popups and top navigation. Scrolling may restart a preview,
but its versioned frame shell is cached. Direct safe HTML never executes scripts.
Links allow HTTP, HTTPS and mailto.

The /markdown sending preference stays in this browser. Each sent message keeps
its own format, including after refresh; /markdown on|off does not change earlier messages.`;
const key = "commonroom.markdown";
export function loadMarkdown(): boolean {
  try {
    return localStorage.getItem(key) === "on";
  } catch {
    return false;
  }
}
export function setMarkdown(input: string): boolean {
  if (!/^\/markdown (on|off)$/u.test(input))
    throw new Error("Usage: /markdown on|off");
  const enabled = input === "/markdown on";
  return saveMarkdown(enabled);
}
export function saveMarkdown(enabled: boolean): boolean {
  try {
    localStorage.setItem(key, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
  return enabled;
}
