import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { frameCsp } from "./src/features/conversation/framePolicy.ts";
import { fileURLToPath } from "node:url";
import {
  defineConfig,
  transformWithEsbuild,
  type Plugin,
  type HttpProxy,
} from "vite";
import preact from "@preact/preset-vite";
// A browser may half-close TCP during logout/navigation while the server still
// has notifications queued. Stop the pipe before those writes reach a closed
// browser socket; actual connection errors still use Vite's normal reporting.
export function configureWebSocketProxy(proxy: HttpProxy.ProxyServer): void {
  proxy.on("proxyReqWs", (request, _incoming, client) => {
    request.once("upgrade", (_response, upstream) => {
      upstream.once("end", () => {
        client.unpipe(upstream);
        upstream.unpipe(client);
        client.end();
        upstream.destroy();
      });
      client.once("end", () => {
        upstream.unpipe(client);
        client.unpipe(upstream);
        upstream.destroy();
        client.destroy();
      });
      if (client.destroyed || client.readableEnded) upstream.destroy();
    });
  });
}

// Expose the emitted module URL so failed imports can be retried with a fresh
// URL. Browsers cache rejected module loads; importing the same URL cannot retry.
function markdownRendererUrl(): Plugin {
  const id = "virtual:markdown-renderer-url";
  const resolved = "\0" + id;
  let development = false;
  return {
    name: "markdown-renderer-url",
    configResolved(config) {
      development = config.command === "serve";
    },
    resolveId(source) {
      return source === id ? resolved : null;
    },
    load(source) {
      if (source !== resolved) return null;
      if (development)
        return 'export default "/src/features/conversation/richMarkdown.ts";';
      const reference = this.emitFile({
        type: "chunk",
        preserveSignature: "strict",
        id: fileURLToPath(
          new URL(
            "./src/features/conversation/richMarkdown.ts",
            import.meta.url,
          ),
        ),
      });
      return `export default import.meta.ROLLUP_FILE_URL_${reference};`;
    },
  };
}

// One self-contained, versioned resource: no JS/CSS subrequests in the sandbox.
function markdownFrame(): Plugin {
  const id = "virtual:markdown-frame-url";
  const resolved = "\0" + id;
  const path = fileURLToPath(
    new URL("./src/markdownFrame.ts", import.meta.url),
  );
  let development = false;
  let document = "";
  let version = "";
  return {
    name: "markdown-frame",
    async configResolved(config) {
      development = config.command === "serve";
      const result = await transformWithEsbuild(
        await readFile(path, "utf8"),
        path,
        { loader: "ts", minify: true, target: "es2022" },
      );
      document =
        '<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="' +
        frameCsp +
        '"><title>HTML preview</title></head><body><script>(()=>{' +
        result.code.replaceAll("</script", "<\\/script") +
        "})();</script></body></html>";
      version = createHash("sha256")
        .update(document)
        .digest("hex")
        .slice(0, 16);
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url?.split("?")[0] !== "/markdown-frame.html") {
          next();
          return;
        }
        response.setHeader("Content-Type", "text/html; charset=utf-8");
        response.setHeader(
          "Content-Security-Policy",
          frameCsp + "; frame-ancestors 'self'",
        );
        response.setHeader(
          "Cache-Control",
          "public, max-age=31536000, immutable",
        );
        response.end(document);
      });
    },
    buildStart() {
      this.addWatchFile(path);
    },
    resolveId(source) {
      return source === id ? resolved : null;
    },
    load(source) {
      if (source !== resolved) return null;
      if (development)
        return `export default "/markdown-frame.html?v=${version}";`;
      const reference = this.emitFile({
        type: "asset",
        fileName: "markdown-frame.html",
        source: document,
      });
      return `export default import.meta.ROLLUP_FILE_URL_${reference} + "?v=${version}";`;
    },
  };
}

// The stable root URL permits an application-scoped service worker under any
// deployment base path. Vite emits its imports with the rest of the bundle.
function mediaWorker(): Plugin {
  const id = "virtual:media-worker-url";
  let development = false;
  return {
    name: "media-worker",
    configResolved(config) {
      development = config.command === "serve";
    },
    resolveId(source) {
      return source === id ? "\0" + id : null;
    },
    load(source) {
      if (source !== "\0" + id) return null;
      if (development) return 'export default "/media-cache.js";';
      const reference = this.emitFile({
        type: "chunk",
        fileName: "media-cache.js",
        id: fileURLToPath(new URL("./src/mediaWorker.ts", import.meta.url)),
      });
      return `export default import.meta.ROLLUP_FILE_URL_${reference};`;
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url?.split("?")[0] !== "/media-cache.js") {
          next();
          return;
        }
        response.setHeader("Content-Type", "text/javascript");
        response.setHeader("Cache-Control", "no-cache");
        response.end('import "/src/mediaWorker.ts";');
      });
    },
  };
}

const packageInfo: unknown = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
);
if (
  !packageInfo ||
  typeof packageInfo !== "object" ||
  !("version" in packageInfo) ||
  typeof packageInfo.version !== "string"
)
  throw new Error("Missing web package version.");
let commit = "unknown";
try {
  commit = execFileSync("git", ["rev-parse", "--verify", "HEAD"], {
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
} catch {
  // Source archives may be built without Git metadata.
}

export default defineConfig({
  define: {
    "import.meta.env.CHAT_VERSION": JSON.stringify(packageInfo.version),
    "import.meta.env.CHAT_COMMIT": JSON.stringify(commit),
  },
  base: "./",
  // Pin the supported Chrome floor while retaining the other Vite 7 targets.
  build: {
    target: ["chrome114", "edge107", "firefox104", "safari16"],
    cssTarget: ["chrome114", "edge107", "firefox104", "safari16"],
  },
  plugins: [preact(), markdownRendererUrl(), markdownFrame(), mediaWorker()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": "http://127.0.0.1:3000",
      "/ws": {
        target: "ws://127.0.0.1:3000",
        ws: true,
        configure: configureWebSocketProxy,
      },
    },
  },
});
