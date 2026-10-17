import { counterSource, isolationSource } from "./counter.mjs";
import { snakeSource } from "./snake.mjs";
import { h, render } from "preact";
import { useRef, useState } from "preact/hooks";
import { MessageList } from "../../src/components/MessageList.tsx";
import "../../src/style.css";
const image =
  "![pixel](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=)";
const source = `<section><b>Safe HTML</b>${"<p>Expandable safe paragraph</p>".repeat(12)}</section><img src="https://invalid.test/raw"><script>window.bad = true</script>\n\n${image}\n\n$x^2$`;
function entry(n, text) {
  return {
    kind: "message",
    key: `m${n}`,
    order: n,
    message: {
      id: `m${n}`,
      sequence: n,
      from: "alice",
      to: null,
      time: 1792166400 + n,
      text,
      markdown: true,
      reactions: {},
      reply: null,
      mentions: [],
    },
  };
}
const initial = Array.from({ length: 1000 }, (_, i) =>
  entry(
    i + 1,
    i === 0
      ? source
      : i === 1
        ? "<script>discard()</script>"
        : i === 2
          ? "`<b>code</b>` <https://example.com> " + image
          : `Message ${i + 1}`,
  ),
);
function Fixture() {
  const [entries, setEntries] = useState(initial);
  const [html, setHtml] = useState(false);
  const [latex, setLatex] = useState(false);
  const [compact, setCompact] = useState(false);
  const [scope, setScope] = useState("demo");
  const [diagnostics, setDiagnostics] = useState("");
  const container = useRef(null),
    handle = useRef(null),
    end = useRef(null),
    atBottom = useRef(false);
  const jump = (sequence) => handle.current?.scrollToSequence(sequence);
  return h(
    "main",
    { style: { maxWidth: compact ? "360px" : "1000px", margin: "16px auto" } },
    h("h1", null, "Markdown rendering checks"),
    h(
      "p",
      null,
      "Isolated fixture: 1,000 messages, no accounts or server data.",
    ),
    h(
      "label",
      null,
      h("input", {
        type: "checkbox",
        checked: html,
        onChange: (e) => setHtml(e.currentTarget.checked),
      }),
      "Allow HTML",
    ),
    h(
      "label",
      null,
      h("input", {
        type: "checkbox",
        checked: latex,
        onChange: (e) => setLatex(e.currentTarget.checked),
      }),
      "Enable LaTeX math",
    ),
    h(
      "div",
      {
        style: {
          display: "flex",
          flexWrap: "wrap",
          gap: "8px",
          margin: "12px 0",
        },
      },
      ...[
        ["Load counter", counterSource],
        ["Load long code", "```html\n" + snakeSource + "\n```"],
        [
          "Load very long code",
          "```html\n" +
            Array.from(
              { length: 180 },
              (_, i) => `<div>HTML source line ${i + 1}</div>`,
            ).join("\n") +
            "\n```",
        ],
        [
          "Load long plain text",
          Array.from({ length: 80 }, (_, i) => `Plain text line ${i + 1}`).join(
            "\n",
          ),
          false,
        ],
        [
          "Load mixed HTML",
          "**Markdown test**\n\n<html>haha</html>\n\n**markdown**\n\nAnother <b>HTML portion</b> with **formatting**.",
        ],
        ["Load isolation checks", isolationSource],
        [
          "Load safe HTML",
          "<section><h3>Safe formatting</h3><p>Hello <b>world</b>.</p><details><summary>More</summary>Safe details</details></section>",
        ],
      ].map(([label, text, markdown = true]) =>
        h(
          "button",
          {
            onClick: () => {
              setEntries(
                initial.map((row, i) =>
                  i === 0
                    ? { ...row, message: { ...row.message, text, markdown } }
                    : row,
                ),
              );
              setScope(label);
              jump(1);
            },
          },
          label,
        ),
      ),
      h(
        "button",
        {
          onClick: () => {
            setEntries(
              initial.map((row, i) =>
                i === 0
                  ? { ...row, message: { ...row.message, text: snakeSource } }
                  : row,
              ),
            );
            setScope("snake");
            jump(1);
          },
        },
        "Load snake game",
      ),
      h(
        "button",
        {
          onClick: () => {
            setEntries(
              initial.map((row, i) =>
                i === initial.length - 1
                  ? { ...row, message: { ...row.message, text: counterSource } }
                  : row,
              ),
            );
            setScope("counter-at-end");
            jump(1000);
          },
        },
        "Counter at end",
      ),
      h(
        "button",
        {
          onClick: async () => {
            const viewport = container.current;
            const article = viewport?.querySelector(".chat-message");
            const panel = article?.querySelector(".message-preview");
            const button = article?.querySelector(".message-expand-toggle");
            const next =
              article?.closest("[data-virtual-key]")?.nextElementSibling;
            if (!viewport || !panel || !button || !next) {
              setDiagnostics("Load long code first.");
              return;
            }
            const frame = () =>
              new Promise((resolve) => requestAnimationFrame(resolve));
            const sample = async () => {
              const heights = [],
                neighbors = [];
              for (let i = 0; i < 36; i++) {
                await frame();
                heights.push(panel.getBoundingClientRect().height);
                neighbors.push(next.getBoundingClientRect().top);
              }
              return { heights, neighbors };
            };
            if (button.getAttribute("aria-expanded") === "true") {
              button.click();
              await sample();
            }
            const collapsed = panel.getBoundingClientRect().height;
            const visibleHeight =
              viewport.getBoundingClientRect().bottom -
              panel.getBoundingClientRect().top;
            button.click();
            const expansion = await sample();
            const full = panel.getBoundingClientRect().height;
            viewport.scrollTop +=
              button.getBoundingClientRect().bottom -
              (viewport.getBoundingClientRect().top + 200);
            await frame();
            await frame();
            const neighbor = next.getBoundingClientRect().top;
            button.click();
            const collapse = await sample();
            const intermediate = (values) =>
              values.filter(
                (value) => value > collapsed + 1 && value < full - 1,
              ).length;
            setDiagnostics(
              JSON.stringify({
                expandedFrames: intermediate(expansion.heights),
                visibleExpansionFrames: expansion.heights.filter(
                  (height) =>
                    height > collapsed + 1 && height < visibleHeight - 1,
                ).length,
                fullHeight: Math.round(full),
                foldedFrames: intermediate(collapse.heights),
                neighborDrift: Math.round(
                  Math.max(
                    ...collapse.neighbors.map((top) =>
                      Math.abs(top - neighbor),
                    ),
                  ),
                ),
                collapsedHeight: Math.round(
                  panel.getBoundingClientRect().height,
                ),
                buttonBorder: getComputedStyle(button).borderWidth,
                buttonFont: getComputedStyle(button).fontSize,
              }),
            );
          },
        },
        "Check fold motion",
      ),
      h("button", { onClick: () => setCompact(!compact) }, "Toggle compact"),
      h("button", { onClick: () => jump(1) }, "Jump to start"),
      h("button", { onClick: () => jump(1000) }, "Jump to end"),
      h(
        "button",
        {
          onClick: () =>
            setEntries((rows) =>
              rows.map((row, i) =>
                i === 0
                  ? {
                      ...row,
                      message: {
                        ...row.message,
                        text: row.message.text + " edited",
                      },
                    }
                  : row,
              ),
            ),
        },
        "Edit first message",
      ),
      h(
        "button",
        { onClick: () => setScope(scope === "demo" ? "other" : "demo") },
        "Switch conversation",
      ),
      h(
        "button",
        {
          onClick: () =>
            setDiagnostics(
              JSON.stringify({
                frameLoads: performance
                  .getEntriesByType("resource")
                  .filter((r) => r.name.includes("markdown-frame.html"))
                  .map((r) => ({ url: r.name, transferSize: r.transferSize })),
                snakeExecuted:
                  document.documentElement.dataset.snakeExecuted === "yes",
                richLoaded: performance
                  .getEntriesByType("resource")
                  .some((r) => r.name.includes("richMarkdown")),
                rows: container.current?.querySelectorAll("[data-virtual-key]")
                  .length,
                rendered: container.current?.querySelectorAll(
                  '[aria-pressed="true"]',
                ).length,
                imageAttributes: Array.from(
                  container.current?.querySelector("img")?.attributes ?? [],
                  (a) => [a.name, a.value],
                ),
                scriptElements: container.current?.querySelectorAll(
                  "script,iframe,video,audio",
                ).length,
              }),
            ),
        },
        "Inspect rendering",
      ),
    ),
    h("output", null, diagnostics),
    h(
      "div",
      { style: { display: "flex", height: "550px", marginTop: "12px" } },
      h(MessageList, {
        motionScope: scope,
        revision: entries[0].message.text.length,
        historyReady: true,
        atBottom,
        entries,
        username: "alice",
        pending: false,
        debug: false,
        allowHtml: html,
        latex,
        permissions: [],
        commands: [],
        unreadBoundary: null,
        onReact: () => {},
        onEdit: async () => true,
        onReply: () => {},
        onDelete: () => {},
        historyElement: container,
        virtualTimeline: handle,
        end,
        onScroll: () => {},
      }),
    ),
  );
}
render(h(Fixture), document.getElementById("app"));
