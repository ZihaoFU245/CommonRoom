import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer as createHttpServer } from "node:http";
import { createConnection } from "node:net";
import { createHash } from "node:crypto";
import { createServer as createViteServer, createLogger } from "vite";
import { configureWebSocketProxy } from "../vite.config.ts";

test(
  "Vite WebSocket proxy stops late notifications after browser TCP half-close",
  { timeout: 15000 },
  async () => {
    const sockets = new Set();
    const errors = [];
    const backend = createHttpServer();
    backend.on("upgrade", (request, socket) => {
      socket.allowHalfOpen = true;
      sockets.add(socket);
      socket.on("error", () => {});
      socket.on("close", () => sockets.delete(socket));
      const accept = createHash("sha1")
        .update(
          request.headers["sec-websocket-key"] +
            "258EAFA5-E914-47DA-95CA-C5AB0DC85B11",
        )
        .digest("base64");
      socket.write(
        `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
      );
      if (request.url === "/ws/server-close") {
        socket.end();
        return;
      }
      socket.on("end", () => {
        // Model a queued notification arriving after a browser navigates away.
        setImmediate(() => {
          if (!socket.destroyed) {
            socket.write(Buffer.from([0x81, 4, ...Buffer.from("late")]));
            socket.end();
          }
        });
      });
    });
    await new Promise((resolve) => backend.listen(0, "127.0.0.1", resolve));
    const logger = createLogger("error");
    logger.error = (message) => errors.push(message);
    const vite = await createViteServer({
      configFile: false,
      root: import.meta.dirname,
      customLogger: logger,
      server: {
        host: "127.0.0.1",
        port: 0,
        proxy: {
          "/ws": {
            target: `ws://127.0.0.1:${backend.address().port}`,
            ws: true,
            configure: configureWebSocketProxy,
          },
        },
      },
    });
    try {
      await vite.listen();
      for (let index = 0; index < 10; index++) {
        await new Promise((resolve, reject) => {
          const client = createConnection({
            host: "127.0.0.1",
            port: vite.httpServer.address().port,
          });
          let response = "";
          client.setTimeout(2000, () =>
            client.destroy(new Error("proxy close timeout")),
          );
          client.on("error", reject);
          client.on("connect", () =>
            client.write(
              "GET /ws HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            ),
          );
          client.on("data", (data) => {
            response += data.toString();
            if (response.includes("\r\n\r\n") && !client.writableEnded)
              client.end();
          });
          client.on("close", () => {
            try {
              assert.match(response, /101 Switching Protocols/);
              resolve();
            } catch (error) {
              reject(error);
            }
          });
        });
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
      await new Promise((resolve, reject) => {
        const client = createConnection({
          host: "127.0.0.1",
          port: vite.httpServer.address().port,
          allowHalfOpen: true,
        });
        client.on("error", reject);
        client.on("data", () => {});
        client.on("connect", () =>
          client.write(
            "GET /ws/server-close HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
          ),
        );
        client.on("end", () => {
          client.end(Buffer.from([0x89, 0]));
        });
        client.on("close", resolve);
      });
      await new Promise((resolve) => setTimeout(resolve, 50));
      assert.deepEqual(errors, []);
    } finally {
      await vite.close();
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => backend.close(resolve));
    }
  },
);
