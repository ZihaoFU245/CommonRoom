// HTTP debug and production HTTPS integration. All fixture data is disposable.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { createHash, X509Certificate } from "node:crypto";
import { createServer as httpServer, request as httpRequest } from "node:http";
import { createServer as httpsServer } from "node:https";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import WebSocket from "ws";

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(predicate, label) {
  const end = Date.now() + 15000;
  while (!(await predicate())) {
    if (Date.now() > end) throw new Error(`Timed out: ${label}`);
    await pause(25);
  }
}
async function listen(server) {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return server.address().port;
}
function proxy(port, forwarded = false) {
  return (req, res) => {
    const upstream = httpRequest(
      {
        host: "127.0.0.1",
        port,
        method: req.method,
        path: req.url,
        headers: {
          ...req.headers,
          ...(forwarded
            ? { "x-forwarded-proto": "https", "x-forwarded-for": "127.0.0.1" }
            : {}),
        },
      },
      (response) => {
        res.writeHead(response.statusCode, response.headers);
        response.pipe(res);
      },
    );
    upstream.on("error", () => {
      res.writeHead(502);
      res.end();
    });
    req.pipe(upstream);
  };
}

export async function startRemoteServer(
  root,
  vitePort,
  wildcard = false,
  sameOrigin = false,
  plainHttp = false,
) {
  const work = await mkdtemp(join(tmpdir(), "commonroom-remote-browser-"));
  const servers = [];
  const connections = new Set();
  const apiRequests = [];
  function recordApi(req) {
    const record = { method: req.method, path: req.url, mediaIds: [] };
    apiRequests.push(record);
    if (req.method === "POST" && req.url.endsWith("/api/media/resolve")) {
      const chunks = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        record.mediaIds = JSON.parse(
          Buffer.concat(chunks).toString("utf8"),
        ).items.map((item) => item.id);
      });
    }
  }
  let apiPort;
  function track(...listeners) {
    for (const server of listeners) {
      servers.push(server);
      server.on("connection", (connection) => {
        connections.add(connection);
        connection.on("close", () => connections.delete(connection));
      });
    }
  }
  let child, socket;
  let logs = "";
  const close = async () => {
    socket?.terminate();
    if (child && child.exitCode === null && child.signalCode === null) {
      const stopped = once(child, "exit");
      child.kill("SIGTERM");
      const force = setTimeout(() => child.kill("SIGKILL"), 5000);
      await stopped;
      clearTimeout(force);
    }
    // closeAllConnections excludes upgraded sockets. Destroy every connection
    // owned by these disposable fixtures so successful browser runs can exit.
    for (const connection of connections) connection.destroy();
    await Promise.all(
      servers.map((server) => new Promise((resolve) => server.close(resolve))),
    );
    await rm(work, { recursive: true, force: true });
  };
  try {
    await promisify(execFile)("openssl", [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      join(work, "key.pem"),
      "-out",
      join(work, "cert.pem"),
      "-days",
      "1",
      "-subj",
      "/CN=commonroom-browser-test",
      "-addext",
      "subjectAltName=DNS:ui.test,DNS:api.test,DNS:store.test",
    ]);
    const tls = {
      key: await readFile(join(work, "key.pem")),
      cert: await readFile(join(work, "cert.pem")),
    };
    const publicServer = (handler) =>
      plainHttp ? httpServer(handler) : httpsServer(tls, handler);
    const scheme = plainHttp ? "http" : "https";
    const ui = publicServer((req, res) => {
      // Never send this isolated browser's default bootstrap to an existing dev backend.
      if (sameOrigin && req.url.startsWith("/server/")) {
        recordApi(req);
        // Match auto/debug.sh's preview: HTTP GETs lack browser Fetch Metadata.
        if (plainHttp && !req.headers.origin && !req.headers["sec-fetch-site"])
          req.headers.origin = uiOrigin;
        proxy(apiPort, !plainHttp)(req, res);
      } else if (req.url.startsWith("/api/")) {
        res.writeHead(401, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "Please log in." }));
      } else proxy(vitePort)(req, res);
    });
    track(ui);
    const uiOrigin = `${scheme}://ui.test:${await listen(ui)}`;
    const objects = new Map();
    const storageRequests = [];
    const storageHandler = async (req, res) => {
      const url = new URL(req.url, "http://storage.test");
      storageRequests.push({
        method: req.method,
        path: url.pathname,
        origin: req.headers.origin,
        cookie: req.headers.cookie,
      });
      if (req.headers.origin === uiOrigin)
        res.setHeader("Access-Control-Allow-Origin", uiOrigin);
      if (req.method === "OPTIONS") {
        res.writeHead(204, {
          "Access-Control-Allow-Methods": "GET, HEAD, PUT",
          "Access-Control-Allow-Headers":
            "content-type, range, x-amz-meta-client-filename, x-amz-meta-client-content-type, x-amz-meta-client-client, x-amz-meta-client-label",
        });
        res.end();
        return;
      }
      if (req.method === "PUT") {
        const source = req.headers["x-amz-copy-source"];
        if (source) {
          const bytes = objects.get(
            `/${decodeURIComponent(source).replace(/^\/+/, "")}`,
          );
          if (!bytes) {
            res.writeHead(404);
            res.end();
            return;
          }
          objects.set(url.pathname, bytes);
          res.writeHead(200, { "Content-Type": "application/xml" });
          res.end("<CopyObjectResult><ETag>test</ETag></CopyObjectResult>");
        } else {
          const chunks = [];
          for await (const chunk of req) chunks.push(chunk);
          objects.set(url.pathname, Buffer.concat(chunks));
          res.writeHead(200);
          res.end();
        }
        return;
      }
      if (req.method === "DELETE") {
        objects.delete(url.pathname);
        res.writeHead(204);
        res.end();
        return;
      }
      const bytes = objects.get(url.pathname);
      if (!bytes) {
        res.writeHead(404);
        res.end();
        return;
      }
      const range = /^bytes=(\d+)-(\d*)$/u.exec(req.headers.range ?? "");
      const start = range ? Number(range[1]) : 0;
      const end = range?.[2]
        ? Math.min(Number(range[2]), bytes.length - 1)
        : bytes.length - 1;
      const body = bytes.subarray(start, end + 1);
      res.setHeader(
        "Content-Type",
        url.searchParams.get("response-content-type") ??
          "application/octet-stream",
      );
      res.setHeader("Cache-Control", "private, no-store");
      res.setHeader("Content-Length", body.length);
      if (url.searchParams.has("response-content-disposition"))
        res.setHeader(
          "Content-Disposition",
          url.searchParams.get("response-content-disposition"),
        );
      if (range)
        res.setHeader("Content-Range", `bytes ${start}-${end}/${bytes.length}`);
      res.writeHead(range ? 206 : 200);
      res.end(req.method === "HEAD" ? undefined : body);
    };
    const internalStorage = httpServer((req, res) => {
      storageHandler(req, res).catch((error) => {
        res.destroy(error);
      });
    });
    const publicStorage = publicServer((req, res) => {
      storageHandler(req, res).catch((error) => {
        res.destroy(error);
      });
    });
    track(internalStorage, publicStorage);
    const internalStorageOrigin = `http://127.0.0.1:${await listen(internalStorage)}`;
    const storageOrigin = `${scheme}://${plainHttp ? "ui.test" : "store.test"}:${await listen(publicStorage)}`;
    const apiProxy = publicServer((req, res) => {
      recordApi(req);
      proxy(apiPort, true)(req, res);
    });
    const upgrade = async (req, incoming, head) => {
      const { connect } = await import("node:net");
      const upstream = connect(apiPort, "127.0.0.1", () => {
        upstream.write(
          `${req.method} ${req.url} HTTP/1.1\r\n${Object.entries({
            ...req.headers,
            "x-forwarded-proto": "https",
            "x-forwarded-for": "127.0.0.1",
          })
            .map(([name, value]) => `${name}: ${value}`)
            .join("\r\n")}\r\n\r\n`,
        );
        if (head.length) upstream.write(head);
        incoming.pipe(upstream).pipe(incoming);
      });
      upstream.on("error", () => incoming.destroy());
      incoming.on("error", () => upstream.destroy());
      incoming.on("close", () => upstream.destroy());
    };
    apiProxy.on("upgrade", upgrade);
    if (sameOrigin) ui.on("upgrade", upgrade);
    track(apiProxy);
    const apiOrigin = `${scheme}://api.test:${await listen(apiProxy)}`;
    const base = `${sameOrigin ? uiOrigin : apiOrigin}/server/`;
    const data = join(work, "data");
    await mkdir(data);
    await mkdir(join(work, "downloads"));
    await writeFile(
      join(data, "config.json"),
      JSON.stringify({
        production: !plainHttp,
        origins: wildcard ? ["*"] : [uiOrigin],
        bind: "127.0.0.1:0",
        base_url: "/server/",
        s3: {
          endpoint: internalStorageOrigin,
          public_endpoint: storageOrigin,
          bucket: "test",
          region: "us-east-1",
          path_style: true,
        },
      }),
    );
    const env = Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith("CHAT_")),
    );
    Object.assign(env, {
      CHAT_DATA: data,
      CHAT_S3_ACCESS_KEY: "test-only",
      CHAT_S3_SECRET_KEY: "test-only-secret",
      RUST_LOG: "chat=info",
    });
    child = spawn(join(root, "../target/debug/chat"), [], {
      cwd: work,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    child.on("error", (error) => {
      logs += error.stack;
    });
    child.stdout.on("data", (chunk) => {
      logs += chunk;
    });
    child.stderr.on("data", (chunk) => {
      logs += chunk;
    });
    await until(() => {
      if (child.exitCode !== null) throw new Error(logs);
      return logs.includes("Chat server ready");
    }, "Rust startup");
    apiPort = Number(/address=127\.0\.0\.1:(\d+)/u.exec(logs)?.[1]);
    assert.ok(apiPort, logs);
    child.stdin.write(
      "/user new alice remote-test-password admin\n/user new bob other-test-password user\n/room new files\n/room add alice #files\n",
    );
    await until(() => logs.includes("alice added to #files"), "accounts");
    if (plainHttp) {
      child.stdin.write("/room new other\n/room add alice #other\n");
      await until(
        () => logs.includes("alice added to #other"),
        "HTTP second room",
      );
    }
    const direct = `http://127.0.0.1:${apiPort}/server/`;
    let cookie;
    async function api(path, body) {
      const response = await fetch(new URL(`api/${path}`, direct), {
        method: body === undefined ? "GET" : "POST",
        headers: {
          Origin: uiOrigin,
          "X-Forwarded-Proto": "https",
          ...(cookie ? { Cookie: cookie } : {}),
          ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      if (!response.ok)
        throw new Error(
          `${path}: ${response.status} ${await response.text()}\n${logs}`,
        );
      return response;
    }
    const login = await api("login", {
      username: "alice",
      password: "remote-test-password",
    });
    cookie = login.headers.get("set-cookie").split(";")[0];
    const account = await (await api("me")).json();
    const room = account.rooms.find((room) => room.name === "files");
    assert.ok(room, JSON.stringify(account));
    const png = Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a1ZkAAAAASUVORK5CYII=",
      "base64",
    );
    const noteBytes = Buffer.from("Downloaded from the remote server.\n");
    async function upload(bytes, filename, avatar = false) {
      const prefix = avatar ? "avatars/uploads" : "media/uploads";
      const ticket = await (
        await api(
          prefix,
          avatar
            ? { size: bytes.length, upload_id: crypto.randomUUID() }
            : { conversation_id: room.id, filename, size: bytes.length },
        )
      ).json();
      const target = new URL(ticket.upload.url);
      const upload = await fetch(
        `${internalStorageOrigin}${target.pathname}${target.search}`,
        { method: "PUT", headers: ticket.upload.headers, body: bytes },
      );
      assert.equal(upload.status, 200);
      return (await api(`${prefix}/${ticket.media_id}/complete`, {})).json();
    }
    const image = await upload(png, "remote.png");
    const note = await upload(noteBytes, "remote-note.txt");
    const html = await upload(
      Buffer.from(
        "<!doctype html><html><body>Remote preview verified</body></html>",
      ),
      "remote-preview.html",
    );
    const wave = Buffer.alloc(4044);
    wave.write("RIFF");
    wave.writeUInt32LE(wave.length - 8, 4);
    wave.write("WAVE", 8);
    wave.write("fmt ", 12);
    wave.writeUInt32LE(16, 16);
    wave.writeUInt16LE(1, 20);
    wave.writeUInt16LE(1, 22);
    wave.writeUInt32LE(8000, 24);
    wave.writeUInt32LE(16000, 28);
    wave.writeUInt16LE(2, 32);
    wave.writeUInt16LE(16, 34);
    wave.write("data", 36);
    wave.writeUInt32LE(4000, 40);
    const audio = await upload(wave, "remote.wav");
    const avatar = await upload(png, "avatar.png", true);
    socket = new WebSocket(`${direct.replace("http:", "ws:")}ws`, {
      headers: {
        Origin: uiOrigin,
        Cookie: cookie,
        "X-Forwarded-Proto": "https",
      },
    });
    await once(socket, "open");
    const frames = [];
    socket.on("message", (raw) => frames.push(JSON.parse(String(raw))));
    await until(
      () => frames.some((frame) => frame.kind === "snapshot"),
      "seed snapshot",
    );
    socket.send(
      JSON.stringify({
        id: 1,
        room: "files",
        text: `![Remote image](${image.path})\n[remote-note.txt](${note.path})\n[remote-preview.html](${html.path} "media:html")\n[remote.wav](${audio.path} "media:audio")`,
        markdown: true,
      }),
    );
    await until(
      () => frames.some((frame) => frame.id === 1),
      "seed attachment message",
    );
    assert.notEqual(
      frames.find((frame) => frame.id === 1).kind,
      "error",
      JSON.stringify(frames),
    );
    return {
      url: uiOrigin,
      base,
      work,
      storageOrigin,
      noteBytes,
      notePath: note.path,
      storageRequests,
      apiRequests,
      sameOrigin,
      plainHttp,
      certificateSpki: createHash("sha256")
        .update(
          new X509Certificate(tls.cert).publicKey.export({
            type: "spki",
            format: "der",
          }),
        )
        .digest("base64"),
      png,
      mediaPaths: [
        image.path,
        new URL(`api/avatars/${avatar.media_id}/file`, base).pathname,
      ],
      close,
      get logs() {
        return logs;
      },
    };
  } catch (error) {
    await close();
    throw error;
  }
}

export async function runRemoteServer({ command, sessionId, fixture, events }) {
  async function evaluate(expression) {
    const result = await command(
      "Runtime.evaluate",
      {
        expression,
        awaitPromise: true,
        returnByValue: true,
        userGesture: true,
      },
      sessionId,
    );
    if (result.exceptionDetails)
      throw new Error(
        result.exceptionDetails.exception?.description ??
          result.exceptionDetails.text,
      );
    return result.result.value;
  }
  async function wait(expression, label) {
    await until(() => evaluate(expression), label);
  }
  await command("Browser.setDownloadBehavior", {
    behavior: "allow",
    downloadPath: join(fixture.work, "downloads"),
    eventsEnabled: true,
  });
  await wait('!!document.querySelector(".login-server-trigger")', "login gear");
  await evaluate('document.querySelector(".login-server-trigger").click()');
  await wait('!!document.querySelector("#login-server-url")', "settings card");
  await evaluate(
    `document.querySelector("#login-server-url").value=${JSON.stringify(fixture.base)}; document.querySelector("#login-server-url").dispatchEvent(new Event("input",{bubbles:true}));`,
  );
  await pause(30);
  await evaluate(
    'document.querySelector(".login-server-card").requestSubmit()',
  );
  await wait('!document.querySelector(".login-server-card")', "saved settings");
  await evaluate(
    'document.querySelector("#username").value="alice"; document.querySelector("#username").dispatchEvent(new Event("input",{bubbles:true})); document.querySelector("#password").value="remote-test-password"; document.querySelector("#password").dispatchEvent(new Event("input",{bubbles:true}));',
  );
  await pause(30);
  await evaluate('document.querySelector(".login-card form").requestSubmit()');
  await wait('!!document.querySelector(".chat-shell")', "cross-site login");
  await wait(
    'document.querySelector(".connection")?.textContent.includes("Connected") || document.querySelector(".connection")?.classList.contains("online")',
    "cross-site WSS",
  );
  await evaluate(
    '[...document.querySelectorAll(".room-link")].find((element)=>element.textContent.includes("files")).click()',
  );
  if (fixture.plainHttp) {
    await runHttpDebug({ command, fixture, events, evaluate, wait });
    return;
  }
  await wait(
    '[...document.querySelectorAll("a")].some((anchor)=>anchor.textContent==="remote-note.txt" && anchor.href.includes("media-download"))',
    "attachment history",
  );
  await wait(
    '[...document.querySelectorAll(".message-markdown img")].some((image)=>image.complete && image.naturalWidth===1)',
    "remote inline image",
  );
  await wait(
    '[...document.querySelectorAll(".user-avatar")].some((image)=>!image.hidden && image.complete && image.naturalWidth===1)',
    "remote avatar",
  );
  await wait(
    "!!navigator.serviceWorker.controller",
    "media worker controls the UI",
  );
  await command("Page.enable", {}, sessionId);
  await command(
    "Page.addScriptToEvaluateOnNewDocument",
    {
      source:
        'window.hardReloadStartedControlled = !!navigator.serviceWorker.controller; Object.defineProperty(AbortSignal, "any", {value:undefined, configurable:true}); Object.defineProperty(AbortSignal, "timeout", {value:undefined, configurable:true});',
    },
    sessionId,
  );
  const storageReads = () =>
    fixture.storageRequests.filter(
      (req) => req.method === "GET" && req.origin === fixture.url,
    ).length;
  const signedLinks = () =>
    fixture.apiRequests.filter((req) => req.path.includes("/link")).length;
  const resolutions = () =>
    fixture.apiRequests.filter((req) =>
      req.mediaIds.includes(fixture.mediaPaths[0].split("/").at(-2)),
    ).length;
  async function hardReload() {
    const previousDocument = await evaluate("performance.timeOrigin");
    await command("Page.reload", { ignoreCache: true }, sessionId);
    await wait(
      `performance.timeOrigin !== ${previousDocument}`,
      "hard reload commits a new document",
    );
    await wait(
      '!!document.querySelector(".chat-shell")',
      "chat after hard reload",
    );
    assert.equal(
      await evaluate("window.hardReloadStartedControlled"),
      false,
      "hard reload starts without service-worker control",
    );
    await wait(
      "!!navigator.serviceWorker.controller",
      "hard reload restores media-cache control",
    );
    await wait(
      '[...document.querySelectorAll(".message-markdown img")].some(image => image.complete && image.naturalWidth === 1)',
      "image after hard reload",
    );
    await wait(
      '[...document.querySelectorAll(".user-avatar")].some(image => !image.hidden && image.complete && image.naturalWidth === 1)',
      "avatar after hard reload",
    );
  }
  await evaluate('caches.delete("commonroom-media")');
  const coldReads = storageReads();
  await hardReload();
  assert.equal(
    storageReads() - coldReads,
    2,
    "cold reload fetches image and avatar once each",
  );
  // Cache fills outlive response consumption; wait for both persistent entries.
  await wait(
    'caches.open("commonroom-media").then(cache => cache.keys()).then(keys => keys.length === 2)',
    "completed image cache fills",
  );
  const readsBefore = storageReads(),
    linksBefore = signedLinks(),
    resolutionsBefore = resolutions();
  await hardReload();
  assert.equal(
    storageReads(),
    readsBefore,
    "warm hard reload transfers no storage bytes",
  );
  assert.equal(
    resolutions(),
    resolutionsBefore,
    "warm image reload needs no read authorization RTT",
  );
  assert.equal(
    signedLinks(),
    linksBefore,
    "warm hard reload skips signed-link APIs",
  );
  // Unique playback queries bypass native image-memory reuse and exercise the
  // actual service-worker/CORS/CacheStorage pipeline on every virtual remount.
  async function remountImages() {
    await evaluate(`Promise.all(${JSON.stringify(fixture.mediaPaths)}.flatMap(path => Array.from({length: 20}, (_, index) => new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = resolve;
      image.onerror = () => reject(new Error("Cached remote image failed"));
      image.src = new URL(path + "?playback=" + index, ${JSON.stringify(fixture.base)}).href;
    })))).then(() => true)`);
  }
  await remountImages();
  assert.equal(
    storageReads(),
    readsBefore,
    "40 warm remounts transfer no storage bytes",
  );
  assert.equal(
    signedLinks(),
    linksBefore,
    "warm remounts skip signed-link APIs",
  );
  // A distinct URL keeps application bootstrap requests from warming the
  // preflight entry measured here.
  const snapshotPath = "/server/api/me?preflight-cache";
  const snapshotUrl = new URL("api/me?preflight-cache", fixture.base).href;
  const preflights = () =>
    fixture.apiRequests.filter(
      (req) => req.method === "OPTIONS" && req.path === snapshotPath,
    ).length;
  const beforePreflight = preflights();
  const snapshots = () =>
    fixture.apiRequests.filter(
      (req) => req.method === "GET" && req.path === snapshotPath,
    ).length;
  const beforeSnapshots = snapshots();
  const requestSnapshot = () =>
    evaluate(`fetch(${JSON.stringify(snapshotUrl)}, {
    credentials: "include", cache: "default", headers: {"X-Chat-Name-Colors": "1"}
  }).then(async r => { if (!r.ok || r.headers.get("Cache-Control") !== "no-store") throw new Error("Snapshot must be fresh and no-store"); await r.json(); return r.status; })`);
  assert.equal(await requestSnapshot(), 200);
  assert.equal(await requestSnapshot(), 200);
  // Exceed the browser's default five-second preflight lifetime. The explicit
  // server max-age must reuse OPTIONS while actual authenticated GETs stay live.
  await pause(6100);
  assert.equal(await requestSnapshot(), 200);
  assert.equal(
    preflights() - beforePreflight,
    fixture.sameOrigin ? 0 : 1,
    "three credentialed reads reuse one preflight beyond five seconds",
  );
  assert.equal(
    snapshots() - beforeSnapshots,
    3,
    "no-store API responses still send every actual GET to the server",
  );
  const cookies = await command("Storage.getCookies");
  const session = cookies.cookies.find(
    (cookie) => cookie.name === "chat_session",
  );
  assert.ok(
    session?.secure &&
      session.httpOnly &&
      session.sameSite === (fixture.sameOrigin ? "Strict" : "None") &&
      session.path === "/server/",
    "session cookie retains the deployment-appropriate security attributes",
  );
  await evaluate(
    '[...document.querySelectorAll(".html-gate button")].find((button)=>button.textContent==="Show text").click()',
  );
  await wait(
    'document.body.textContent.includes("Remote preview verified")',
    "remote HTML text preview",
  );
  await wait('!!document.querySelector("audio[src]")', "signed audio source");
  await evaluate('document.querySelector("audio").play()');
  await wait(
    'document.querySelector("audio").currentTime > 0',
    "remote audio playback",
  );

  await wait('!!document.querySelector("input[type=file]")', "upload control");
  await evaluate(
    'const files=new DataTransfer(); files.items.add(new File(["Browser upload verified"],"browser-upload.txt",{type:"text/plain"})); const picker=document.querySelector("input[type=file]"); picker.files=files.files; picker.dispatchEvent(new Event("change",{bubbles:true}));',
  );
  await wait(
    'document.querySelector(".media-upload-status")?.textContent.includes("Attachment ready")',
    "cross-site browser upload",
  );
  await evaluate('document.querySelector(".composer").requestSubmit()');
  await wait(
    '[...document.querySelectorAll(".message-markdown a")].some((anchor)=>anchor.textContent==="browser-upload.txt")',
    "published browser upload",
  );

  await evaluate(
    'const input=document.querySelector(".composer textarea"); input.value="Browser remote message verified"; input.dispatchEvent(new Event("input",{bubbles:true}));',
  );
  await pause(30);
  await evaluate('document.querySelector(".composer").requestSubmit()');
  await wait(
    'document.querySelector(".message-list")?.textContent.includes("Browser remote message verified")',
    "message acknowledgement",
  );
  await evaluate(
    '[...document.querySelectorAll("a")].find((anchor)=>anchor.textContent==="remote-note.txt").click()',
  );
  await until(
    () =>
      events.some(
        (event) =>
          event.method === "Browser.downloadProgress" &&
          event.params.state === "completed",
      ),
    "remote download completion",
  );
  assert.deepEqual(
    await readFile(join(fixture.work, "downloads", "remote-note.txt")),
    fixture.noteBytes,
  );
  assert.equal(
    new URL(
      events.find((event) => event.method === "Browser.downloadWillBegin")
        .params.url,
    ).origin,
    fixture.url,
  );
  await wait(
    `caches.open("commonroom-media").then(cache => cache.match(new URL(${JSON.stringify(fixture.notePath)}, ${JSON.stringify(fixture.base)}).href)).then(Boolean)`,
    "download fill persisted",
  );
  const readsAfterDownload = storageReads(),
    linksAfterDownload = signedLinks();
  const completedDownloads = () =>
    events.filter(
      (event) =>
        event.method === "Browser.downloadProgress" &&
        event.params.state === "completed",
    ).length;
  const downloadsBefore = completedDownloads();
  await evaluate(
    '[...document.querySelectorAll("a")].find(anchor => anchor.textContent === "remote-note.txt").click()',
  );
  await until(
    () => completedDownloads() > downloadsBefore,
    "cached native download completes",
  );
  assert.equal(
    storageReads(),
    readsAfterDownload,
    "repeated download reuses cached bytes",
  );
  assert.equal(
    signedLinks(),
    linksAfterDownload,
    "repeated download skips signed-link API",
  );
  assert.ok(
    fixture.storageRequests
      .filter((req) => req.method === "GET")
      .every((req) => !req.cookie),
    "storage never receives a chat session cookie",
  );
  await command("Page.reload", {}, sessionId);
  await wait(
    '!!document.querySelector(".chat-shell")',
    "saved server and session after reload",
  );
  await wait(
    'document.querySelector(".connection")?.classList.contains("online")',
    "WSS after reload",
  );
  const readsAfterReload = storageReads(),
    linksAfterReload = signedLinks();
  await remountImages();
  assert.equal(storageReads(), readsAfterReload, "remote bytes survive reload");
  assert.equal(
    signedLinks(),
    linksAfterReload,
    "reload uses persistent canonical keys",
  );
  await evaluate('document.querySelector(".signout").click()');
  await wait('!!document.querySelector(".login-server-trigger")', "logout UI");
  assert.equal(
    await evaluate(`fetch(${JSON.stringify(snapshotUrl)}, {
    credentials: "include", cache: "default", headers: {"X-Chat-Name-Colors": "1"}
  }).then(r => r.status)`),
    401,
    "cached preflight approval cannot authorize a logged-out snapshot",
  );
  assert.equal(
    preflights() - beforePreflight,
    fixture.sameOrigin ? 0 : 1,
    "logout still checks authentication with the preflight cached",
  );
  const readsAfterLogout = storageReads();
  assert.equal(
    await evaluate(
      `fetch(new URL(${JSON.stringify(fixture.mediaPaths[0])}, ${JSON.stringify(fixture.base)}), {credentials:"include", cache:"no-store"}).then(r => r.status)`,
    ),
    401,
    "logout denies warm cached bytes",
  );
  assert.equal(storageReads(), readsAfterLogout);
  await evaluate(
    `fetch(new URL("api/login", ${JSON.stringify(fixture.base)}), {method:"POST", credentials:"include", headers:{"Content-Type":"application/json"}, body:JSON.stringify({username:"bob", password:"other-test-password"})}).then(r => {if (!r.ok) throw new Error("Second account login failed");})`,
  );
  const switchedStatus = await evaluate(
    `fetch(new URL(${JSON.stringify(fixture.mediaPaths[0])}, ${JSON.stringify(fixture.base)}), {credentials:"include", cache:"no-store"}).then(r => r.status)`,
  );
  assert.ok(
    [403, 404].includes(switchedStatus),
    "another account cannot read the first account's cached attachment",
  );
  assert.equal(storageReads(), readsAfterLogout);
  await evaluate(
    `fetch(new URL("api/logout", ${JSON.stringify(fixture.base)}), {method:"POST", credentials:"include"})`,
  );
  await command("Page.reload", {}, sessionId);
  await wait('!!document.querySelector(".login-server-trigger")', "logout");
  await evaluate('document.querySelector(".login-server-trigger").click()');
  await wait(
    `document.querySelector("#login-server-url")?.value === ${JSON.stringify(fixture.base)}`,
    "server persists after logout",
  );
  console.log(
    `PASS real HTTPS ${fixture.sameOrigin ? "same-origin" : "cross-site"} server: settings, login, WSS, messages, avatar, image, audio, HTML preview, upload, cached downloads, cold/warm hard reload and logout`,
  );
}

async function runHttpDebug({ fixture, events, evaluate, wait }) {
  assert.equal(
    await evaluate("window.isSecureContext"),
    false,
    "non-loopback HTTP must exercise the actual browser restrictions",
  );
  assert.equal(await evaluate("typeof crypto.randomUUID"), "undefined");
  assert.equal(await evaluate("typeof navigator.serviceWorker"), "undefined");
  await wait(
    '[...document.querySelectorAll(".message-markdown img")].some(image => image.complete && image.naturalWidth === 1)',
    "HTTP inline image",
  );
  await wait(
    '[...document.querySelectorAll(".user-avatar")].some(image => !image.hidden && image.complete && image.naturalWidth === 1)',
    "HTTP avatar",
  );
  assert.equal(
    await evaluate('document.body.textContent.includes("Retry image")'),
    false,
  );
  assert.equal(
    new URL(fixture.url).hostname,
    new URL(fixture.storageOrigin).hostname,
  );
  assert.notEqual(
    new URL(fixture.url).port,
    new URL(fixture.storageOrigin).port,
  );
  const imagePath = fixture.mediaPaths[0];
  const imageLink = imagePath.replace(/\/file$/u, "/link");
  const imageInfo = imagePath.replace(/\/file$/u, "/info");
  const links = () =>
    fixture.apiRequests.filter((request) => request.path === imageLink).length;
  const auth = () =>
    fixture.apiRequests.filter((request) => request.path === imageInfo).length;
  const reads = () =>
    fixture.storageRequests.filter(
      (request) => request.method === "GET" && request.origin === fixture.url,
    ).length;
  const resolutions = () =>
    fixture.apiRequests.filter((request) =>
      request.mediaIds.includes(imagePath.split("/").at(-2)),
    ).length;
  const before = {
    links: links(),
    auth: auth(),
    reads: reads(),
    resolutions: resolutions(),
  };
  for (let pass = 0; pass < 3; pass++) {
    await evaluate(
      '[...document.querySelectorAll(".room-link")].find(button => button.textContent.includes("other")).click()',
    );
    await wait(
      '!document.querySelector(".message-markdown img")',
      "old image unmounted",
    );
    await evaluate(
      '[...document.querySelectorAll(".room-link")].find(button => button.textContent.includes("files")).click()',
    );
    await wait(
      '[...document.querySelectorAll(".message-markdown img")].some(image => image.src.startsWith("blob:") && image.complete && image.naturalWidth === 1)',
      "HTTP image restored from memory",
    );
  }
  assert.equal(
    reads(),
    before.reads,
    "repeated row remounts cannot download storage again",
  );
  assert.equal(
    links(),
    before.links,
    "cached images do not request another signature",
  );
  assert.equal(
    auth(),
    before.auth,
    "valid read leases avoid per-remount info checks",
  );
  assert.equal(
    resolutions(),
    before.resolutions,
    "warm image remounts need no read resolution RTT",
  );

  await evaluate(
    '[...document.querySelectorAll(".html-gate button")].find(button => button.textContent === "Show text").click()',
  );
  await wait(
    'document.body.textContent.includes("Remote preview verified")',
    "HTTP HTML text preview",
  );
  await wait('!!document.querySelector("audio[src]")', "HTTP audio source");
  await evaluate('document.querySelector("audio").play()');
  await wait(
    'document.querySelector("audio").currentTime > 0',
    "HTTP audio playback",
  );
  await wait(
    `[...document.querySelectorAll("a")].some(anchor => anchor.textContent === "remote-note.txt" && new URL(anchor.href).origin === ${JSON.stringify(fixture.storageOrigin)})`,
    "HTTP direct download link",
  );
  await evaluate(
    '[...document.querySelectorAll("a")].find(anchor => anchor.textContent === "remote-note.txt").click()',
  );
  await until(
    () =>
      events.some(
        (event) =>
          event.method === "Browser.downloadProgress" &&
          event.params.state === "completed",
      ),
    "HTTP native download",
  );
  assert.deepEqual(
    await readFile(join(fixture.work, "downloads", "remote-note.txt")),
    fixture.noteBytes,
  );
  await evaluate(
    'const transfer = new DataTransfer(); transfer.items.add(new File(["Debug upload verified"], "debug-upload.txt", {type:"text/plain"})); const input = document.querySelector(".composer input[type=file]"); input.files = transfer.files; input.dispatchEvent(new Event("change", {bubbles:true}));',
  );
  await wait(
    'document.querySelector(".media-upload-status")?.textContent.includes("Attachment ready")',
    "HTTP attachment upload",
  );
  await evaluate('document.querySelector(".composer").requestSubmit()');
  await wait(
    '[...document.querySelectorAll(".message-markdown a")].some(anchor => anchor.textContent === "debug-upload.txt")',
    "HTTP upload message",
  );
  await evaluate(`document.querySelector('[aria-label="Settings"]').click()`);
  await wait(
    `!!document.querySelector('[aria-label="Change avatar"]')`,
    "avatar settings",
  );
  await evaluate(
    `document.querySelector('[aria-label="Change avatar"]').click()`,
  );
  await wait('!!document.querySelector(".avatar-menu")', "avatar upload menu");
  await evaluate(
    `const avatarTransfer = new DataTransfer(); avatarTransfer.items.add(new File([Uint8Array.from(atob(${JSON.stringify(fixture.png.toString("base64"))}), c => c.charCodeAt(0))], "debug-avatar.png", {type:"image/png"})); const avatarInput = document.querySelector(".avatar-settings input[type=file]"); avatarInput.files = avatarTransfer.files; avatarInput.dispatchEvent(new Event("change", {bubbles:true}));`,
  );
  await wait(
    '[...document.querySelectorAll(".avatar-crop button")].some(button => button.textContent === "Confirm and upload" && !button.disabled)',
    "HTTP avatar crop",
  );
  await evaluate(
    '[...document.querySelectorAll(".avatar-crop button")].find(button => button.textContent === "Confirm and upload").click()',
  );
  await wait(
    'document.querySelector(".avatar-settings")?.textContent.includes("Avatar updated.")',
    "HTTP avatar allocation/PUT/completion without randomUUID",
  );
  await wait(
    '[...document.querySelectorAll(".account-avatar img")].some(image => !image.hidden && image.complete && image.naturalWidth === 256)',
    "uploaded HTTP avatar displays",
  );
  assert.ok(
    fixture.apiRequests.some(
      (request) =>
        request.method === "POST" &&
        request.path === "/server/api/avatars/uploads",
    ),
  );
  assert.ok(
    fixture.storageRequests.filter((request) => request.method === "PUT")
      .length >= 2,
  );
  assert.ok(
    fixture.storageRequests.every((request) => !request.cookie),
    "storage never receives chat cookies",
  );
  console.log(
    "PASS real non-loopback HTTP debug: login, WS, images, avatars, HTML, audio, downloads, attachment upload and avatar crop/upload without secure-context APIs",
  );
}
