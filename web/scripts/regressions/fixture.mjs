import { h, render } from "preact";
import { useHistory } from "../../src/hooks/useHistory.ts";
import { useReadTracking } from "../../src/hooks/useReadTracking.ts";
import { useCommands } from "../../src/hooks/useCommands.ts";
import { MediaCache } from "../../src/features/conversation/mediaCache.ts";
import { Settings } from "../../src/components/Settings.tsx";
import { api } from "../../src/api/client.ts";
import { defaultSeeds } from "../../src/features/preferences/palette.ts";
import "../../src/style.css";

const tests = document.querySelector("#tests");
const results = [];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function check(condition, message) {
  if (!condition) throw new Error(message);
}
async function until(predicate) {
  const end = Date.now() + 5000;
  while (!predicate()) {
    if (Date.now() > end)
      throw new Error("Timed out waiting for browser state");
    await sleep(10);
  }
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function probe(hook) {
  let result;
  function Probe() {
    result = hook();
    return h("span", { "data-probe": true });
  }
  const element = document.createElement("div");
  tests.append(element);
  return {
    update() {
      render(h(Probe), element);
    },
    get current() {
      return result;
    },
    close() {
      render(null, element);
      element.remove();
    },
  };
}
async function test(name, run) {
  const originals = { ...api };
  try {
    await run();
    results.push({ name });
  } catch (error) {
    results.push({ name, error: error.stack ?? String(error) });
  } finally {
    Object.assign(api, originals);
    render(null, tests);
    tests.replaceChildren();
  }
}
const message = (sequence) => ({
  id: `m-${sequence}`,
  sequence,
  time: sequence,
  from: "alice",
  to: null,
  text: "hello",
  reactions: {},
  reply: null,
  mentions: [],
});
const empty = {
  kind: "snapshot",
  username: "alice",
  groups: [],
  permissions: [],
  account_access: { id: "alice-id", permissions: [], commands: [] },
  private_access: {},
  private_permissions: [],
  private_commands: [],
  policy_revision: 0,
  admin: false,
  users: [],
  online: [],
  rooms: [],
  direct: [],
  private_peers: [],
  commands: [],
  available_rooms: [],
  unread: {},
};

await test("real Preact clears cancelled loading when switching to cached history", async () => {
  const tail = [message(1)];
  const state = {
    ...empty,
    rooms: [
      { name: "a", id: "a-id", messages: [] },
      { name: "b", id: "b-id", messages: tail },
    ],
    unread: {
      a: { revision: 1, through: 100, oldest: 1 },
      b: { revision: 1, through: 1, oldest: 1 },
    },
  };
  let selected = "a",
    cancelled = false;
  api.history = (_view, signal) =>
    new Promise((_, reject) =>
      signal.addEventListener(
        "abort",
        () => {
          cancelled = true;
          reject(signal.reason);
        },
        { once: true },
      ),
    );
  const currentState = { current: state },
    ledger = { current: { sequence: 0, messages: new Map() } },
    bottom = { current: true },
    timeline = { current: null };
  const p = probe(() =>
    useHistory(
      state,
      selected,
      state.rooms.find((room) => room.name === selected).messages,
      currentState,
      ledger,
      timeline,
      bottom,
    ),
  );
  try {
    p.update();
    await until(() => p.current?.loading);
    selected = "b";
    p.update();
    await until(
      () => cancelled && p.current?.historyReady && !p.current.loading,
    );
    check(p.current.messages[0].id === "m-1", "cached history is visible");
  } finally {
    p.close();
  }
});

await test("slow read acknowledgements drain the latest conversation without another scroll", async () => {
  const pending = deferred(),
    reads = [];
  api.read = (view, through) => {
    reads.push([view, through]);
    return reads.length === 1 ? pending.promise : Promise.resolve({ ok: true });
  };
  const viewport = document.createElement("section");
  viewport.style.cssText =
    "position:fixed;top:0;left:0;width:300px;height:200px;overflow:auto";
  const row = document.createElement("article");
  row.style.height = "40px";
  row.dataset.sequence = "10";
  viewport.append(row);
  tests.append(viewport);
  const focus = document.hasFocus;
  document.hasFocus = () => true;
  const context = {
    selected: "a",
    conversationId: "a-id",
    unread: { first: null },
    messages: [message(10)],
    historyReady: true,
    output: [],
    cleared: {},
    resetCleared() {},
    settingsOpen: false,
    menu: false,
    status: "online",
    loadSequence() {},
    virtualTimeline: { current: null },
    atBottom: { current: true },
    historyElement: { current: viewport },
  };
  const p = probe(() => useReadTracking(context));
  try {
    p.update();
    await until(() => reads.length === 1);
    context.selected = "b";
    context.conversationId = "b-id";
    context.messages = [message(20)];
    row.dataset.sequence = "20";
    p.update();
    await sleep(350);
    pending.resolve({ ok: true });
    await until(() => reads.length === 2);
    check(
      reads[1][0] === "b" && reads[1][1] === 20,
      "new view must be acknowledged after old request completes",
    );
  } finally {
    pending.resolve({ ok: true });
    p.close();
    document.hasFocus = focus;
  }
});

await test("command owners preserve drafts across delayed acknowledgements and failed sends", async () => {
  api.mediaConfig = async () => ({ enabled: false, max_size: 0 });
  const frames = [];
  const socket = {
    current: {
      readyState: WebSocket.OPEN,
      send(raw) {
        frames.push(JSON.parse(raw));
      },
    },
  };
  const state = {
    ...empty,
    permissions: ["w:message.create"],
    rooms: [
      {
        name: "team",
        id: "team-id",
        members: ["alice"],
        messages: [],
        permissions: ["w:message.create"],
        commands: [],
      },
    ],
  };
  const input = document.createElement("textarea");
  tests.append(input);
  const context = {
    botCommands: [],
    selected: "team",
    direct: false,
    consoleView: false,
    peer: null,
    state,
    room: state.rooms[0],
    socket,
    currentState: { current: state },
    input: { current: input },
    desiredRoom: { current: null },
    setSelected() {},
    append() {
      return "local";
    },
    complete() {},
    clearView() {},
    logout() {},
  };
  const p = probe(() => useCommands(context));
  try {
    p.update();
    p.current.edit("first");
    await until(() => p.current.draft === "first");
    p.current.send(new Event("submit"));
    await until(() => frames.length === 1 && p.current.outgoing.length === 1);
    p.current.edit("new draft");
    await until(() => p.current.draft === "new draft");
    p.current.receive({ kind: "notice", id: frames[0].id, text: "sent" });
    await until(() => p.current.outgoing.length === 0);
    check(
      p.current.draft === "new draft",
      "old confirmation cannot clear a newer draft",
    );
    p.current.send(new Event("submit"));
    await until(() => frames.length === 2 && p.current.outgoing.length === 1);
    p.current.disconnect();
    await until(() => p.current.outgoing[0]?.error);
    p.current.restoreOutgoing(p.current.outgoing[0].key);
    await until(
      () => p.current.draft === "new draft" && p.current.outgoing.length === 0,
    );
    p.current.edit("/markdown invalid");
    await until(() => p.current.draft === "/markdown invalid");
    p.current.send(new Event("submit"));
    await sleep(50);
    check(
      p.current.draft === "/markdown invalid",
      "invalid local preferences keep their draft for correction",
    );
  } finally {
    p.close();
  }
});

await test("settings panel switches clear credentials and ignore late account results", async () => {
  const pending = deferred(),
    commands = [];
  const state = {
    ...empty,
    account_access: {
      id: "alice-id",
      commands: [{ name: "/user" }],
      permissions: ["w:account.rename.own", "w:account.password.own"],
    },
  };
  const props = {
    state,
    pending: false,
    onAccountCommand(text) {
      commands.push(text);
      return pending.promise;
    },
    aliases: {},
    onSaveAlias() {},
    onRemoveAlias() {},
    fonts: { chat: 16, ui: 14 },
    theme: "light",
    seeds: defaultSeeds,
    markdown: false,
    onMarkdownChange() {},
    latex: false,
    onLatexChange() {},
    allowHtml: false,
    onAllowHtmlChange() {},
    onChange() {},
    onThemeChange() {},
    onSeedsChange() {},
    onClose() {},
  };
  function click(text) {
    const button = [...tests.querySelectorAll("button")].find(
      (button) => button.textContent.trim() === text,
    );
    check(button, `missing ${text} button`);
    button.click();
  }
  function type(id, value) {
    const input = tests.querySelector(`#${id}`);
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }
  render(h(Settings, props), tests);
  try {
    await until(() => tests.querySelector("dialog")?.open);
    click("Security");
    await until(() => tests.querySelector("#markdown-html-description"));
    click("Change");
    await until(() => tests.querySelector("#account-old-password"));
    type("account-old-password", "old");
    type("account-new-password", "new");
    type("account-confirm-password", "new");
    await sleep(30);
    tests
      .querySelector("#account-password-editor")
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await until(() => commands.length === 1);
    click("Appearance");
    await until(() => tests.querySelector("#chat-font-size"));
    pending.resolve({
      kind: "notice",
      id: 1,
      text: "secret operation completed",
    });
    await sleep(50);
    click("Security");
    await until(() => tests.querySelector("#markdown-html-description"));
    check(
      !tests.textContent.includes("secret operation completed"),
      "late results cannot enter a replacement panel",
    );
    click("Change");
    await until(() => tests.querySelector("#account-old-password"));
    check(
      [...tests.querySelectorAll('input[type="password"]')].every(
        (input) => input.value === "",
      ),
      "credentials must clear when leaving security",
    );
    click("Account");
    await until(
      () =>
        tests.querySelector(".account-setting-detail strong")?.textContent ===
        "Username",
    );
    click("Edit");
    await until(() => tests.querySelector("#account-name"));
    click("Appearance");
    await until(() => tests.querySelector("#chat-font-size"));
    click("Account");
    await until(
      () =>
        tests.querySelector(".account-setting-detail strong")?.textContent ===
        "Username",
    );
    check(
      !tests.querySelector("#account-name"),
      "account editor closes on tab changes",
    );
  } finally {
    pending.resolve({ kind: "notice", id: 1, text: "done" });
    render(null, tests);
  }
});

await test("real CacheStorage shares cold fills while checking each caller's access", async () => {
  await caches.delete("commonroom-media-v2");
  const url = new URL(
    "api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file",
    location.origin,
  );
  let downloads = 0,
    authorizations = 0,
    denied = false;
  const work = [];
  const media = new MediaCache(caches, async (address) => {
    if (String(address).endsWith("/info")) {
      authorizations++;
      return denied
        ? new Response(null, { status: 403 })
        : Response.json({
            size: 6,
            content_type: "image/png",
            filename: "test.png",
          });
    }
    downloads++;
    await sleep(100);
    return new Response("abcdef", { headers: { "Content-Length": "6" } });
  });
  try {
    const responses = await Promise.all(
      Array.from({ length: 16 }, () =>
        media.respond(new Request(url), url, (promise) => work.push(promise)),
      ),
    );
    const texts = await Promise.all(
      responses.map((response) => response.text()),
    );
    await Promise.all(work);
    check(
      texts.every((text) => text === "abcdef"),
      "every concurrent caller receives the complete bytes",
    );
    check(
      downloads === 1 && authorizations === 16,
      `expected one download and 16 checks, got ${downloads}/${authorizations}`,
    );
    denied = true;
    check(
      (
        await media.respond(new Request(url), url, (promise) =>
          work.push(promise),
        )
      ).status === 403,
      "cached bytes remain inaccessible after revocation",
    );
  } finally {
    await caches.delete("commonroom-media-v2");
  }
});

document.querySelector("#results").textContent = JSON.stringify(
  results,
  null,
  2,
);
window.browserTestResults = results;
