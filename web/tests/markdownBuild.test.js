import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

test("production HTML renderer keeps its exports and stays outside the initial import graph", async () => {
  const result = await build({
    root: fileURLToPath(new URL("..", import.meta.url)),
    configFile: fileURLToPath(new URL("../vite.config.ts", import.meta.url)),
    logLevel: "silent",
    build: { write: false },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap(
    (bundle) => bundle.output,
  );
  const html = outputs.find(
    (output) => output.type === "asset" && output.fileName === "index.html",
  );
  assert.ok(html);
  const document = String(html.source);
  assert.ok(document.includes("script-src 'self'"));
  assert.ok(!document.includes("script-src 'self' 'unsafe-inline'"));
  assert.ok(document.includes("object-src 'none'"));
  assert.ok(
    document.includes("connect-src 'self' https: wss:"),
    "fallback allows configured HTTPS object storage",
  );
  assert.ok(document.includes('name="referrer" content="no-referrer"'));
  assert.ok(
    !outputs.some((output) => output.fileName.startsWith("scripts/")),
    "test harnesses must not ship",
  );
  const chunks = outputs.filter((output) => output.type === "chunk");
  const rich = chunks.find((chunk) => chunk.fileName.includes("richMarkdown"));
  assert.ok(rich);
  assert.ok(
    rich.exports.includes("markdownNodes"),
    "runtime import needs the exported sanitizer",
  );
  const entry = chunks.find(
    (chunk) => chunk.isEntry && chunk.facadeModuleId?.endsWith("index.html"),
  );
  assert.ok(entry);
  const visited = new Set();
  function walk(chunk) {
    if (visited.has(chunk.fileName)) return;
    visited.add(chunk.fileName);
    assert.ok(
      !Object.keys(chunk.modules).some((path) =>
        /(?:markdown-it|plugin-tex|parse5|katex|richMarkdown|HtmlMarkdownGate|MarkdownFrame)/u.test(
          path,
        ),
      ),
      "initial imports must stay lightweight",
    );
    for (const name of chunk.imports) {
      const dependency = chunks.find((item) => item.fileName === name);
      if (dependency) walk(dependency);
    }
  }
  walk(entry);
  assert.ok(!visited.has(rich.fileName));
  const frame = outputs.find(
    (output) =>
      output.type === "asset" && output.fileName === "markdown-frame.html",
  );
  assert.ok(frame, "standalone frame is packaged");
  assert.ok(
    String(frame.source).includes('http-equiv="Content-Security-Policy"'),
  );
  assert.ok(
    !String(frame.source).includes("<script src="),
    "frame has no script subrequests",
  );
  assert.ok(!String(frame.source).includes("allow-same-origin"));
  const frameChunk = chunks.find((chunk) =>
    Object.keys(chunk.modules).some((path) =>
      path.endsWith("/components/MarkdownFrame.tsx"),
    ),
  );
  assert.ok(frameChunk);
  assert.ok(!visited.has(frameChunk.fileName));
});

test("debug artifacts allow local HTTP object storage without allowing inline scripts", async () => {
  const result = await build({
    root: fileURLToPath(new URL("..", import.meta.url)),
    configFile: fileURLToPath(new URL("../vite.config.ts", import.meta.url)),
    mode: "development",
    logLevel: "silent",
    build: { write: false },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap(
    (bundle) => bundle.output,
  );
  const html = outputs.find(
    (output) => output.type === "asset" && output.fileName === "index.html",
  );
  assert.ok(html);
  const document = String(html.source);
  assert.ok(document.includes("connect-src 'self' http: https: ws: wss:"));
  assert.ok(document.includes("media-src 'self' data: blob: http: https:"));
  assert.ok(document.includes("script-src 'self'"));
  assert.ok(!document.includes("script-src 'self' 'unsafe-inline'"));
});
