import assert from "node:assert/strict";
import { test } from "node:test";
import {
  normalizeServerUrl,
  savedServerUrl,
  saveServerUrl,
  serverBaseUrl,
} from "../src/api/server.ts";
import { api } from "../src/api/client.ts";

test("server addresses preserve prefixes and default to HTTPS", () => {
  assert.equal(
    normalizeServerUrl(" chat.example.com/server "),
    "https://chat.example.com/server/",
  );
  assert.equal(
    normalizeServerUrl("http://localhost:3000/chat///"),
    "http://localhost:3000/chat/",
  );
  for (const value of [
    "",
    "https://",
    "ftp://chat.example.com",
    "https://user:password@chat.example.com",
    "https://chat.example.com/?token=x",
    "https://chat.example.com/#x",
  ]) {
    assert.throws(() => normalizeServerUrl(value));
  }
});

test("server selection persists per UI deployment and directs credentialed API requests", async (t) => {
  const store = new Map();
  const originalDocument = globalThis.document;
  const originalStorage = globalThis.localStorage;
  t.after(() => {
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: originalStorage,
    });
  });
  globalThis.document = {
    baseURI: "https://ui.example.com/chat/",
  };
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, value),
    },
  });
  assert.equal(savedServerUrl(), "https://ui.example.com/chat/");
  saveServerUrl("chat.example.com/server");
  assert.equal(savedServerUrl(), "https://chat.example.com/server/");
  assert.equal(serverBaseUrl(), "https://chat.example.com/server/");
  t.mock.method(globalThis, "fetch", async (url, options) => {
    assert.equal(url.href, "https://chat.example.com/server/api/login");
    assert.equal(options.credentials, "include");
    return Response.json({ ok: true });
  });
  await api.login("alice", "password");
  saveServerUrl("");
  assert.equal(savedServerUrl(), "https://ui.example.com/chat/");
  globalThis.document.baseURI = "https://ui.example.com/other/";
  assert.equal(savedServerUrl(), "https://ui.example.com/other/");
  globalThis.localStorage.setItem = () => {
    throw new Error("denied");
  };
  assert.throws(() => saveServerUrl("chat.example.com"), /could not be saved/);
});
