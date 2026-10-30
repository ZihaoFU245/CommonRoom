import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer, request } from "node:http";
import { preview } from "vite";
import { debugReadOrigin } from "../vite.config.ts";

test("debug preview supports HTTP reads without overriding browser origin checks", async () => {
  const backend = createServer((incoming, response) => {
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify(incoming.headers));
  });
  await new Promise((resolve) => backend.listen(0, "127.0.0.1", resolve));
  let ui;
  try {
    ui = await preview({
      configFile: false,
      root: import.meta.dirname,
      logLevel: "silent",
      plugins: [debugReadOrigin("http://devbox.example.ts.net:5173")],
      preview: {
        host: "127.0.0.1",
        port: 0,
        allowedHosts: ["devbox.example.ts.net"],
        proxy: { "/api": `http://127.0.0.1:${backend.address().port}` },
      },
    });
    const get = (headers, method = "GET") =>
      new Promise((resolve, reject) => {
        const req = request(
          {
            hostname: "127.0.0.1",
            port: ui.httpServer.address().port,
            path: "/api/me",
            method,
            headers: { host: "devbox.example.ts.net:5173", ...headers },
          },
          (response) => {
            let body = "";
            response.on("data", (chunk) => (body += chunk));
            response.on("end", () =>
              resolve({
                status: response.statusCode,
                headers: response.headers,
                body,
              }),
            );
          },
        );
        req.on("error", reject);
        req.end();
      });
    const plain = await get({});
    assert.equal(
      JSON.parse(plain.body).origin,
      "http://devbox.example.ts.net:5173",
    );
    for (const headers of [
      { origin: "http://evil.example" },
      { "sec-fetch-site": "cross-site" },
      { "sec-fetch-site": "same-site" },
      { "sec-fetch-site": "same-origin" },
      { host: "127.0.0.1:5173" },
    ]) {
      const result = await get(headers);
      assert.equal(JSON.parse(result.body).origin, headers.origin);
    }
    const post = await get({}, "POST");
    assert.equal(JSON.parse(post.body).origin, undefined);
    const crossOrigin = await get({ origin: "http://evil.example" });
    assert.equal(crossOrigin.headers["access-control-allow-origin"], undefined);
    const rejected = await get({ host: "evil.example:5173" });
    assert.equal(rejected.status, 403);
  } finally {
    if (ui) await new Promise((resolve) => ui.httpServer.close(resolve));
    await new Promise((resolve) => backend.close(resolve));
  }
});
