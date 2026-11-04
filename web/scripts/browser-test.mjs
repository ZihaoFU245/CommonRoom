import { spawn } from "node:child_process";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import { build, createServer, preview } from "vite";
import WebSocket from "ws";
import {
  startRemoteServer,
  runRemoteServer,
} from "./remote-server-browser.mjs";

// Use an installed browser and an isolated profile; no backend or account data.
const root = fileURLToPath(new URL("../", import.meta.url));
const remote = process.argv[2] === "--remote-server";
const plainHttp = remote && process.argv[3] === "http";
const candidates = process.env.CHROME_BIN
  ? [process.env.CHROME_BIN]
  : [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/usr/bin/google-chrome",
      "/usr/bin/chromium",
      "/usr/bin/chromium-browser",
    ];
let executable;
for (const candidate of candidates) {
  try {
    await access(candidate);
    executable = candidate;
    break;
  } catch {
    /* Try the next installation. */
  }
}
if (!executable)
  throw new Error(
    "Install Chrome/Chromium or set CHROME_BIN to its executable.",
  );
const profile = await mkdtemp(join(tmpdir(), "chatroom-web-browser-"));
let server, browser, socket, fixture;
const events = [];
const pending = new Map();
let serial = 0;
try {
  const built = remote && process.argv.includes("--built");
  if (built) {
    const outDir = join(profile, "dist");
    await build({
      root,
      configFile: resolve(root, "vite.config.ts"),
      logLevel: "error",
      ...(plainHttp ? { mode: "development" } : {}),
      build: { outDir },
    });
    const serving = await preview({
      root,
      configFile: resolve(root, "vite.config.ts"),
      build: { outDir },
      preview: {
        host: "127.0.0.1",
        port: 0,
        strictPort: false,
        allowedHosts: ["ui.test"],
      },
    });
    server = {
      httpServer: serving.httpServer,
      close: () =>
        new Promise((resolveClose) => serving.httpServer.close(resolveClose)),
    };
  } else {
    server = await createServer({
      root,
      configFile: resolve(root, "vite.config.ts"),
      server: {
        host: "127.0.0.1",
        port: 0,
        strictPort: false,
        ...(remote ? { allowedHosts: ["ui.test"], hmr: false } : {}),
      },
      logLevel: "error",
    });
    await server.listen();
  }
  const address = server.httpServer.address();
  if (!address || typeof address === "string")
    throw new Error("Missing Vite test listener.");
  if (remote)
    fixture = await startRemoteServer(
      root,
      address.port,
      process.argv[3] === "*",
      plainHttp || process.argv[3] === "same-origin",
      plainHttp,
    );
  browser = spawn(
    executable,
    [
      "--headless=new",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-networking",
      ...(remote
        ? [
            "--ignore-certificate-errors",
            "--no-proxy-server",
            "--host-resolver-rules=MAP ui.test 127.0.0.1, MAP api.test 127.0.0.1, MAP store.test 127.0.0.1",
            "--disable-features=ThirdPartyCookiePhaseout,TrackingProtection3pcd",
          ]
        : []),
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "about:blank",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  const endpoint = await new Promise((resolveEndpoint, reject) => {
    let log = "";
    const timer = setTimeout(
      () => reject(new Error(`Chrome startup timed out: ${log}`)),
      15000,
    );
    browser.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    browser.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`Chrome exited (${code}): ${log}`));
    });
    browser.stderr.on("data", (chunk) => {
      log = (log + chunk).slice(-8000);
      const match = /DevTools listening on (ws:\/\/\S+)/u.exec(log);
      if (match) {
        clearTimeout(timer);
        resolveEndpoint(match[1]);
      }
    });
  });
  socket = new WebSocket(endpoint);
  await once(socket, "open");
  socket.on("message", (raw) => {
    const message = JSON.parse(String(raw));
    if (message.method) events.push(message);
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    clearTimeout(request.timer);
    if (message.error) request.reject(new Error(message.error.message));
    else request.resolve(message.result);
  });
  function command(method, params = {}, sessionId) {
    return new Promise((resolveCommand, reject) => {
      const id = ++serial;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 15000);
      pending.set(id, { resolve: resolveCommand, reject, timer });
      socket.send(
        JSON.stringify({
          id,
          method,
          params,
          ...(sessionId ? { sessionId } : {}),
        }),
      );
    });
  }
  const { targetId } = await command("Target.createTarget", {
    url:
      fixture?.url ??
      `http://127.0.0.1:${address.port}/scripts/regressions/index.html${process.argv[2] ? `?${new URLSearchParams({ test: process.argv[2] })}` : ""}`,
  });
  const { sessionId } = await command("Target.attachToTarget", {
    targetId,
    flatten: true,
  });
  await command(
    "Emulation.setDeviceMetricsOverride",
    {
      width: 1280,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    },
    sessionId,
  );
  await command("Page.bringToFront", {}, sessionId);
  if (fixture) {
    try {
      await runRemoteServer({ command, sessionId, fixture, events });
    } catch (error) {
      const diagnostic = await command(
        "Runtime.evaluate",
        { expression: "document.body.innerText", returnByValue: true },
        sessionId,
      );
      throw new Error(
        `${error.stack}\nBrowser: ${diagnostic.result.value}\nServer: ${fixture.logs}`,
        { cause: error },
      );
    }
  } else {
    const deadline = Date.now() + 90000;
    let result;
    while (Date.now() < deadline) {
      const evaluation = await command(
        "Runtime.evaluate",
        {
          expression: "window.browserTestResults ?? null",
          returnByValue: true,
        },
        sessionId,
      );
      result = evaluation.result.value;
      if (result) break;
      await new Promise((resolveWait) => setTimeout(resolveWait, 100));
    }
    if (!result)
      throw new Error(
        "Browser regressions timed out before producing results.",
      );
    for (const test of result)
      console.log(
        `${test.error ? "FAIL" : "PASS"} ${test.name}${test.error ? `\n${test.error}` : ""}`,
      );
    if (result.some((test) => test.error)) process.exitCode = 1;
  }
} finally {
  for (const request of pending.values()) {
    clearTimeout(request.timer);
    request.reject(new Error("Browser runner closed"));
  }
  socket?.terminate();
  if (browser && browser.exitCode === null) {
    const exited = once(browser, "exit");
    browser.kill();
    const force = setTimeout(() => browser.kill("SIGKILL"), 2000);
    await exited;
    clearTimeout(force);
  }
  await fixture?.close();
  server?.httpServer?.closeAllConnections();
  await server?.close();
  await rm(profile, { recursive: true, force: true });
}
