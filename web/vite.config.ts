import { defineConfig, type HttpProxy } from "vite";
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

export default defineConfig({
  base: "./",
  plugins: [preact()],
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
