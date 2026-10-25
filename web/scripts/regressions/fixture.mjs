import { h, render } from "preact";
import { useHistory } from "../../src/hooks/useHistory.ts";
import { useSettingsMotion } from "../../src/hooks/useSettingsMotion.ts";
import { useReadTracking } from "../../src/hooks/useReadTracking.ts";
import { useCommands } from "../../src/hooks/useCommands.ts";
import { MediaCache } from "../../src/features/conversation/mediaCache.ts";
import { MessageList } from "../../src/components/MessageList.tsx";
import { Chat } from "../../src/components/Chat.tsx";
import { Login } from "../../src/components/Login.tsx";
import { Settings } from "../../src/components/Settings.tsx";
import { MarkdownContent } from "../../src/components/MarkdownContent.tsx";
import { markdownNodes } from "../../src/features/conversation/markdown.ts";
import { api } from "../../src/api/client.ts";
import {
  defaultSeeds,
  contrast,
} from "../../src/features/preferences/palette.ts";
import { applyAppearance } from "../../src/features/preferences/appearance.ts";
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

await test("settings motion follows reduced-motion changes and cancels active transitions", async () => {
  const original = window.matchMedia;
  const listeners = new Set();
  let reduced = true,
    show = false;
  window.matchMedia = (query) =>
    query === "(prefers-reduced-motion: reduce)"
      ? {
          get matches() {
            return reduced;
          },
          addEventListener(_type, listener) {
            listeners.add(listener);
          },
          removeEventListener(_type, listener) {
            listeners.delete(listener);
          },
        }
      : original.call(window, query);
  function Motion() {
    const container = useSettingsMotion();
    return h(
      "div",
      { ref: container },
      show ? h("span", { id: "motion-probe" }, "Options") : null,
    );
  }
  try {
    render(h(Motion), tests);
    await sleep(70);
    show = true;
    render(h(Motion), tests);
    await sleep(30);
    check(
      tests.querySelector("#motion-probe").getAnimations().length === 0,
      "reduced motion skips option animations",
    );
    show = false;
    render(h(Motion), tests);
    await sleep(30);
    reduced = false;
    for (const listener of listeners) listener();
    await sleep(70);
    show = true;
    render(h(Motion), tests);
    await until(() =>
      tests
        .querySelector("#motion-probe")
        ?.getAnimations()
        .some((animation) => animation.playState === "running"),
    );
    reduced = true;
    for (const listener of listeners) listener();
    check(
      !tests
        .querySelector("#motion-probe")
        .getAnimations()
        .some((animation) => animation.playState === "running"),
      "enabling reduced motion cancels in-flight transitions",
    );
  } finally {
    render(null, tests);
    window.matchMedia = original;
  }
});

await test("same-minute message bars join without gaps and break on minute, author and unread changes", async () => {
  const start = Math.floor(Date.now() / 60000) * 60;
  let entries = [
    { ...message(1), from: "alice", time: start + 5 },
    {
      ...message(2),
      from: "alice",
      time: start + 25,
      reactions: { "👍": ["bob"] },
    },
    {
      ...message(3),
      from: "alice",
      time: start + 59,
      reply: { id: "m-1", from: "alice", text: "Reply in the same minute" },
    },
    { ...message(4), from: "alice", time: start + 60 },
    { ...message(5), from: "bob", time: start + 61 },
    { ...message(6), from: "alice", time: start + 62 },
  ].map((item) => ({
    kind: "message",
    key: item.id,
    order: item.sequence,
    message: item,
  }));
  let bars = true,
    unread = null;
  const history = { current: null },
    timeline = { current: null },
    end = { current: null },
    bottom = { current: false };
  function draw() {
    render(
      h(
        "div",
        {
          class: bars ? "show-message-bars" : "",
          style: { display: "flex", flexDirection: "column", height: "650px" },
        },
        h(MessageList, {
          motionScope: "bar-run-test",
          revision: 1,
          historyReady: true,
          entries,
          username: "alice",
          pending: false,
          debug: false,
          allowHtml: false,
          latex: false,
          messageBars: bars,
          permissions: [
            "r:message.read",
            "w:message.react",
            "w:message.create",
          ],
          commands: [{ name: "/react" }, { name: "/reply" }],
          unreadBoundary: unread,
          atBottom: bottom,
          historyElement: history,
          virtualTimeline: timeline,
          end,
          onReact() {},
          onReply() {},
          onDelete() {},
          onEdit: async () => true,
          onScroll() {},
        }),
      ),
      tests,
    );
  }
  const row = (id) => tests.querySelector(`#message-m-${id}`);
  const bar = (id) =>
    row(id).querySelector(".message-content").getBoundingClientRect();
  draw();
  await until(() => row(6));
  for (const [before, after] of [
    [1, 2],
    [2, 3],
  ]) {
    check(
      Math.abs(bar(before).bottom - bar(after).top) <= 0.5,
      `bars ${before} and ${after} meet exactly, without opacity overlap`,
    );
    check(bar(before).left === bar(after).left, "connected bar segments align");
  }
  check(
    row(2).classList.contains("grouped"),
    "same-minute continuation keeps its compact header",
  );
  check(
    !row(4).classList.contains("grouped") &&
      row(4).querySelector(".message-meta"),
    "next minute starts a fresh header",
  );
  for (const [before, after] of [
    [3, 4],
    [4, 5],
    [5, 6],
  ])
    check(
      bar(after).top > bar(before).bottom + 1,
      `new bar starts between ${before} and ${after}`,
    );
  unread = 2;
  draw();
  await until(() => tests.querySelector(".unread-divider"));
  check(
    !row(1).classList.contains("message-bar-continues") &&
      !row(2).classList.contains("message-bar-continued"),
    "unread divider breaks the line",
  );
  entries = entries.filter((entry) => entry.message.sequence !== 2);
  unread = null;
  draw();
  await until(() => !row(2));
  await until(() => Math.abs(bar(1).bottom - bar(3).top) <= 0.5);
  bars = false;
  draw();
  await until(() => row(4).classList.contains("grouped"));
  check(
    getComputedStyle(row(1).querySelector(".message-content"))
      .borderLeftWidth === "0px",
    "disabled bars preserve the original transcript grouping and layout",
  );
});

for (const view of ["team", "@command", "@direct:bob"]) {
  await test(`normal users can keep composing after denied debug commands in ${view}`, async () => {
    const savedHash = location.hash;
    history.replaceState(null, "", `#room=${encodeURIComponent(view)}`);
    const OriginalSocket = window.WebSocket;
    const frames = [];
    const state = {
      ...empty,
      commands: [
        {
          name: "/help",
          usage: "/help",
          description: "Help",
          requirements: "",
          section: "General",
          admin: false,
          console: false,
        },
      ],
      rooms: [
        {
          name: "team",
          id: "team-id",
          owner: "owner",
          members: ["alice"],
          permissions: ["r:message.read", "w:message.create"],
          commands: [],
          messages: Array.from({ length: 1000 }, (_, i) => message(i + 1)),
        },
      ],
      private_peers: ["bob"],
      direct: [{ ...message(1001), to: "bob" }],
      private_access: {
        "@direct:bob": {
          id: "private-id",
          permissions: ["r:message.read", "w:message.create"],
          commands: [],
        },
      },
      unread: {
        team: { revision: 1, count: 0, first: null, oldest: 1, through: 1000 },
      },
    };
    class TestSocket {
      static OPEN = 1;
      static CLOSED = 3;
      readyState = 1;
      constructor() {
        queueMicrotask(() => {
          this.onopen?.();
          this.onmessage?.({ data: JSON.stringify(state) });
        });
      }
      send(raw) {
        const frame = JSON.parse(raw);
        frames.push(frame);
        queueMicrotask(() =>
          this.onmessage?.({
            data: JSON.stringify({
              kind: "notice",
              id: frame.id,
              text: "sent",
            }),
          }),
        );
      }
      close() {
        this.readyState = 3;
      }
    }
    api.botCommands = async (id) => ({ conversation_id: id, bots: [] });
    api.mediaConfig = async () => ({ enabled: false, max_size: 0 });
    api.read = async () => ({ ok: true });
    api.me = async () => state;
    api.history = async () => ({
      view: "team",
      messages: state.rooms[0].messages,
      revision: 1,
      has_before: false,
      has_after: false,
    });
    window.WebSocket = TestSocket;
    try {
      render(
        h(Chat, {
          initial: state,
          onLogout() {},
          appearance: {
            theme: "light",
            seeds: defaultSeeds,
            setTheme() {},
            setSeeds() {},
          },
        }),
        tests,
      );
      await until(() => tests.querySelector(".composer textarea"));
      const input = tests.querySelector(".composer textarea");
      input.value = "/debug on";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await sleep(50);
      tests
        .querySelector(".composer")
        .dispatchEvent(
          new Event("submit", { bubbles: true, cancelable: true }),
        );
      await until(() => tests.querySelector(".console-output.error"));
      check(frames.length === 0, "missing command grant is rejected locally");
      const nextText = view === "@command" ? "/help" : "still responsive";
      input.value = nextText;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await sleep(50);
      tests
        .querySelector(".composer")
        .dispatchEvent(
          new Event("submit", { bubbles: true, cancelable: true }),
        );
      await until(() => frames.length === 1);
      check(
        frames[0].text.includes(nextText),
        "submission sends after rejection",
      );
    } finally {
      render(null, tests);
      window.WebSocket = OriginalSocket;
      history.replaceState(null, "", savedHash || location.pathname);
    }
  });
}

await test("chat colors update for newly disclosed live and paged authors and reset in place", async () => {
  const OriginalSocket = window.WebSocket;
  let socket;
  const preferenceKeys = [
    "commonroom-name-colors",
    "commonroom-name-borders",
    "commonroom-message-bars",
    "commonroom-message-bar-opacity",
  ];
  const savedPreferences = preferenceKeys.map((key) =>
    localStorage.getItem(key),
  );
  for (const key of preferenceKeys) localStorage.removeItem(key);
  const requests = [];
  function wire(value, enabled) {
    const copy = structuredClone(value);
    if (!enabled) {
      delete copy.colors;
      delete copy.color_version;
      delete copy.name_colors;
      for (const room of copy.rooms ?? []) {
        delete room.colors;
        delete room.name_colors;
      }
    }
    return copy;
  }
  class TestSocket {
    static OPEN = 1;
    static CLOSED = 3;
    readyState = 1;
    constructor(url) {
      this.colors = new URL(url).searchParams.get("colors") === "1";
      socket = this;
      queueMicrotask(() => {
        this.onopen?.();
        this.receive(state);
      });
    }
    receive(value) {
      this.onmessage?.({ data: JSON.stringify(wire(value, this.colors)) });
    }
    send() {}
    close() {
      this.readyState = 3;
    }
  }
  const current = {
    ...message(100),
    reply: { id: "quoted", from: "bob", text: "Quoted message" },
  };
  const old = { ...message(1), from: "carol" };
  const live = { ...message(101), from: "bob" };
  let state = {
    ...empty,
    permissions: [],
    colors: { alice: "purple" },
    color_version: "instance:1",
    rooms: [
      {
        name: "team",
        id: "stable",
        owner: "alice",
        permissions: ["r:message.read", "w:account.color.own"],
        commands: [],
        members: [],
        colors: {},
        messages: [current],
      },
    ],
    unread: {
      team: { revision: 1, count: 0, first: null, oldest: 1, through: 100 },
    },
  };
  let includeOld = false;
  api.botCommands = async (id) => ({ conversation_id: id, bots: [] });
  api.read = async () => ({ ok: true });
  api.me = async (enabled) => wire(state, enabled);
  api.history = async (_view, _signal, range, enabled) => {
    requests.push(enabled);
    includeOld ||= range.before !== undefined;
    const reset = state.color_version === "instance:2";
    return wire(
      {
        view: "team",
        messages: [
          ...(includeOld ? [old] : []),
          current,
          ...(state.unread.team.through === 101 ? [live] : []),
        ],
        revision: state.unread.team.revision,
        has_before: !includeOld,
        has_after: false,
        name_colors: {
          version: state.color_version,
          values: {
            alice: null,
            ...(includeOld ? { carol: reset ? null : "teal" } : {}),
            ...(state.unread.team.through === 101
              ? { bob: reset ? null : "red" }
              : {}),
          },
        },
      },
      enabled,
    );
  };
  window.WebSocket = TestSocket;
  const author = (name) =>
    [...tests.querySelectorAll(".message-author")].find(
      (element) => element.textContent === name,
    );
  try {
    render(
      h(Chat, {
        initial: state,
        onLogout() {},
        appearance: {
          theme: "light",
          seeds: defaultSeeds,
          setTheme() {},
          setSeeds() {},
        },
      }),
      tests,
    );
    await until(
      () =>
        socket &&
        [...tests.querySelectorAll("button")].some(
          (b) => b.textContent === "Older messages" && !b.disabled,
        ),
    );
    check(
      !socket.colors && requests.every((enabled) => !enabled),
      "default requests omit color opt-in",
    );
    check(
      !author("alice").hasAttribute("data-name-color"),
      "name colors default off",
    );
    check(
      getComputedStyle(author("alice")).borderLeftWidth === "0px",
      "name borders default off",
    );
    check(
      getComputedStyle(tests.querySelector(".message-body")).borderLeftWidth ===
        "0px",
      "message side bars default off",
    );
    check(
      getComputedStyle(tests.querySelector(".reply-quote")).borderLeftWidth ===
        "4px",
      "original reply quote bar survives with add-ons off",
    );
    check(
      !performance
        .getEntriesByType("resource")
        .some((entry) => entry.name.includes("NameColorSettings.tsx")),
      "color picker stays unloaded while disabled",
    );
    async function preferences() {
      if (!tests.querySelector('[aria-label="Settings"]')) {
        tests.querySelector('[aria-label="Open navigation"]').click();
        await until(() => tests.querySelector('[aria-label="Settings"]'));
      }
      tests.querySelector('[aria-label="Settings"]').click();
      await until(() => tests.querySelector(".settings-dialog[open]"));
      [...tests.querySelectorAll(".settings-navigation button")]
        .find((button) => button.textContent === "Preferences")
        .click();
      await until(() =>
        tests.querySelector(
          '[aria-describedby="name-colors-preference-description"]',
        ),
      );
    }
    await preferences();
    const colorToggle = tests.querySelector(
      '[aria-describedby="name-colors-preference-description"]',
    );
    const barsToggle = tests.querySelector(
      '[aria-describedby="message-bars-preference-description"]',
    );
    const bordersToggle = tests.querySelector(
      '[aria-describedby="name-borders-preference-description"]',
    );
    check(
      !colorToggle.checked && !barsToggle.checked && !bordersToggle.checked,
      "all new preferences start unchecked",
    );
    await sleep(70);
    colorToggle.click();
    await until(
      () => socket.colors && tests.querySelector("#name-color-scope"),
    );
    check(
      performance
        .getEntriesByType("resource")
        .some((entry) => entry.name.includes("NameColorSettings.tsx")),
      "enabling colors loads the picker lazily",
    );
    const motion = !window.matchMedia("(prefers-reduced-motion: reduce)")
      .matches;
    if (motion)
      await until(() =>
        tests
          .querySelector(".name-color-controls")
          ?.getAnimations()
          .some((animation) => animation.playState === "running"),
      );
    const scopeToggle = tests.querySelector("#name-color-scope");
    check(
      scopeToggle.tagName === "BUTTON",
      "scope selector uses the themed custom control",
    );
    await sleep(70);
    scopeToggle.click();
    await until(
      () => document.activeElement?.getAttribute("aria-selected") === "true",
    );
    const scopes = tests.querySelector("#name-color-scopes");
    if (motion)
      await until(() =>
        scopes
          .getAnimations()
          .some((animation) => animation.playState === "running"),
      );
    check(
      getComputedStyle(scopes).backgroundColor ===
        getComputedStyle(tests.querySelector(".settings-dialog"))
          .backgroundColor,
      "scope menu uses the settings surface",
    );
    document.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    );
    check(
      document.activeElement.textContent.trim() === "#team",
      "scope menu supports arrow-key navigation",
    );
    document.activeElement.click();
    await until(() => scopeToggle.textContent.trim() === "#team");
    check(
      document.activeElement === scopeToggle,
      "scope selection restores trigger focus",
    );
    await until(() => !tests.querySelector("#name-color-scopes"));
    const colorPickerToggle = tests.querySelector(
      ".name-color-toggle:not(.name-scope-toggle)",
    );
    colorPickerToggle.click();
    await until(() => tests.querySelector("#name-color-options"));
    const picker = tests.querySelector("#name-color-options");
    if (motion)
      await until(() =>
        picker
          .getAnimations()
          .some((animation) => animation.playState === "running"),
      );
    picker.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    await until(
      () => colorPickerToggle.getAttribute("aria-expanded") === "false",
    );
    check(
      document.activeElement === colorPickerToggle &&
        tests.querySelector(".settings-dialog[open]"),
      "picker Escape returns focus without closing Settings",
    );

    barsToggle.click();
    bordersToggle.click();
    await until(
      () => getComputedStyle(author("alice")).borderLeftWidth === "1px",
    );
    check(
      getComputedStyle(tests.querySelector(".message-content"))
        .borderLeftWidth === "3px",
      "message side bars can be enabled independently",
    );
    const opacity = tests.querySelector("#message-bar-opacity");
    check(
      opacity?.value === "50",
      "message bars start at a softer 50% opacity",
    );
    const nameColor = getComputedStyle(author("alice")).color;
    const quoteColor = getComputedStyle(
      tests.querySelector(".reply-quote"),
    ).borderLeftColor;
    const opacityCanvas = document.createElement("canvas");
    opacityCanvas.width = opacityCanvas.height = 1;
    const opacityContext = opacityCanvas.getContext("2d");
    for (const value of [20, 0, 100, 50]) {
      opacity.value = String(value);
      opacity.dispatchEvent(new Event("input", { bubbles: true }));
      await until(
        () =>
          tests
            .querySelector(".chat-shell")
            .style.getPropertyValue("--message-bar-opacity") === `${value}%`,
      );
      opacityContext.clearRect(0, 0, 1, 1);
      opacityContext.fillStyle = getComputedStyle(
        tests.querySelector(".message-content"),
      ).borderLeftColor;
      opacityContext.fillRect(0, 0, 1, 1);
      const alpha = opacityContext.getImageData(0, 0, 1, 1).data[3];
      check(
        Math.abs(alpha - value * 2.55) <= 1,
        `bar paints at ${value}% opacity`,
      );
      check(
        getComputedStyle(author("alice")).color === nameColor,
        "opacity does not fade author text",
      );
      check(
        getComputedStyle(tests.querySelector(".reply-quote"))
          .borderLeftColor === quoteColor,
        "opacity does not fade the reply quote bar",
      );
      check(
        localStorage.getItem("commonroom-message-bar-opacity") ===
          String(value),
        "opacity preference is saved",
      );
      check(
        opacity.getAttribute("aria-valuetext") === `${value}%`,
        "slider announces its percentage",
      );
    }
    const outerBar = tests
      .querySelector(".message-content")
      .getBoundingClientRect();
    const quoteBar = tests
      .querySelector(".reply-quote")
      .getBoundingClientRect();
    const messageBody = tests
      .querySelector(".message-body")
      .getBoundingClientRect();
    check(
      quoteBar.left >= outerBar.left + 14,
      "reply bar is inset from the full message bar",
    );
    check(
      quoteBar.top >= outerBar.top && messageBody.bottom <= outerBar.bottom + 1,
      "outer bar spans reply and body",
    );
    check(
      getComputedStyle(tests.querySelector(".message-body")).borderLeftWidth ===
        "0px",
      "message body has no duplicate bar",
    );
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const context = canvas.getContext("2d");
    function hex(color) {
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      return (
        "#" +
        [...context.getImageData(0, 0, 1, 1).data]
          .slice(0, 3)
          .map((channel) => channel.toString(16).padStart(2, "0"))
          .join("")
      );
    }
    for (const [theme, base, accent] of [
      ["light", "#f9f9f7", "#cc7d5e"],
      ["dark", "#181c26", "#cc7d5e"],
      ["custom", "#0f2018", "#3ddc84"],
      ["custom", "#fdf6e3", "#ffe066"],
      ["custom", "#767676", "#ffffff"],
    ]) {
      applyAppearance(theme, false, { base, accent });
      const badge = getComputedStyle(author("alice"));
      check(
        contrast(hex(badge.color), hex(badge.backgroundColor)) >= 4.5,
        `${theme} ${base}: badge text remains readable`,
      );
      check(
        parseFloat(badge.paddingLeft) <= 5 && badge.borderRadius === "4px",
        "name badge remains restrained across themes",
      );
      scopeToggle.click();
      await until(() => tests.querySelector("#name-color-scopes:not([inert])"));
      const menu = tests.querySelector("#name-color-scopes:not([inert])");
      check(
        getComputedStyle(menu).backgroundColor ===
          getComputedStyle(tests.querySelector(".settings-dialog"))
            .backgroundColor,
        `${theme}: scope menu follows the current surface`,
      );
      menu.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      await until(() => !tests.querySelector("#name-color-scopes"));
    }
    applyAppearance("light", false, defaultSeeds);

    barsToggle.click();
    bordersToggle.click();
    await until(
      () => getComputedStyle(author("alice")).borderLeftWidth === "0px",
    );
    check(
      getComputedStyle(tests.querySelector(".reply-quote")).borderLeftWidth ===
        "4px",
      "turning message bars off retains reply quote bar",
    );
    await until(() => !tests.querySelector("#message-bar-opacity"));
    tests.querySelector('[aria-label="Close settings"]').click();
    state = {
      ...state,
      rooms: [{ ...state.rooms[0], messages: [current, live] }],
      unread: { team: { ...state.unread.team, revision: 2, through: 101 } },
    };
    socket.receive({
      kind: "conversation",
      view: "team",
      id: "stable",
      messages: [current, live],
      unread: state.unread.team,
      name_colors: {
        version: state.color_version,
        values: { alice: null, bob: "red" },
      },
    });
    await until(() => author("bob")?.dataset.nameColor === "red");
    check(
      getComputedStyle(author("bob")).color === "rgb(220, 38, 38)",
      "live author paints its scoped red",
    );
    const older = [...tests.querySelectorAll("button")].find(
      (b) => b.textContent === "Older messages",
    );
    older.click();
    await until(() => author("carol")?.dataset.nameColor === "teal");
    check(
      getComputedStyle(author("carol")).color === "rgb(15, 118, 110)",
      "paged author paints teal",
    );
    state = {
      ...state,
      color_version: "instance:2",
      rooms: [{ ...state.rooms[0], colors: {} }],
    };
    socket.receive(state);
    await until(
      () => author("carol") && !author("carol").hasAttribute("data-name-color"),
    );
    check(
      !author("bob")?.hasAttribute("data-name-color"),
      "reset removes cached live author color",
    );
    await preferences();
    const afterEnabled = requests.length;
    tests
      .querySelector('[aria-describedby="name-colors-preference-description"]')
      .click();
    await until(() => !socket.colors);
    tests.querySelector('[aria-label="Close settings"]').click();
    await until(() => !tests.querySelector(".name-color-setting"));
    check(
      requests.slice(afterEnabled).every((enabled) => !enabled),
      "disabled requests stop fetching colors",
    );
  } finally {
    render(null, tests);
    window.WebSocket = OriginalSocket;
    applyAppearance("light", false, defaultSeeds);
    preferenceKeys.forEach((key, i) => {
      const value = savedPreferences[i];
      if (value === null) localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    });
  }
});

await test("color picker previews locally, applies once and preserves keyboard focus through saves", async () => {
  const { NameColorSettings } =
    await import("../../src/components/settings/NameColorSettings.tsx");
  let state = {
    ...empty,
    permissions: [],
    colors: { alice: "red" },
  };
  const writes = [];
  let saving;
  function draw() {
    render(
      h(
        "div",
        { class: "settings-content" },
        h(NameColorSettings, {
          state,
          onNameColorChange(color, room) {
            writes.push({ color, room });
            saving = deferred();
            return saving.promise;
          },
        }),
        h("button", { id: "elsewhere" }, "Elsewhere"),
      ),
      tests,
    );
  }
  draw();
  const trigger = tests.querySelector(
    ".name-color-toggle:not(.name-scope-toggle)",
  );
  const panel = () => tests.querySelector("#name-color-options:not([inert])");
  async function open() {
    trigger.focus();
    trigger.click();
    await until(panel);
  }
  const action = (label) =>
    [...panel().querySelectorAll("button")].find(
      (button) => button.textContent === label,
    );
  await open();
  for (const color of ["blue", "green", "pink", "teal", "violet", "orange"]) {
    const option = panel().querySelector(
      `.name-color-option[data-name-color="${color}"]`,
    );
    option.focus();
    option.click();
    await until(
      () =>
        panel().querySelector(".name-color-preview strong").dataset
          .nameColor === color,
    );
  }
  check(
    writes.length === 0,
    "comparing more than four swatches never writes to the server",
  );
  check(
    state.colors.alice === "red",
    "preview leaves the saved profile unchanged",
  );
  action("Cancel").click();
  await until(() => !panel());
  check(document.activeElement === trigger, "Cancel restores focus");
  await open();
  check(
    panel()
      .querySelector('.name-color-option[data-name-color="red"]')
      .getAttribute("aria-pressed") === "true",
    "cancelled draft is discarded",
  );
  panel().querySelector('.name-color-option[data-name-color="blue"]').click();
  await until(
    () =>
      panel()
        .querySelector('.name-color-option[data-name-color="blue"]')
        .getAttribute("aria-pressed") === "true",
  );
  action("Apply").click();
  await until(() => trigger.getAttribute("aria-disabled") === "true");
  check(
    writes.length === 1 &&
      writes[0].color === "blue" &&
      writes[0].room === null,
    "Apply submits exactly one final color",
  );
  check(
    document.activeElement === trigger && !trigger.disabled,
    "saving keeps trigger focusable and focused",
  );
  trigger.click();
  check(
    !panel(),
    "saving blocks reopening without disabling the focused trigger",
  );
  state = { ...state, colors: { alice: "blue" } };
  draw();
  saving.resolve();
  await until(() => trigger.getAttribute("aria-disabled") === "false");
  check(document.activeElement === trigger, "success retains focus");
  await open();
  panel().querySelector('.name-color-option[data-name-color="pink"]').click();
  await until(
    () =>
      panel()
        .querySelector('.name-color-option[data-name-color="pink"]')
        .getAttribute("aria-pressed") === "true",
  );
  action("Apply").click();
  await until(() => trigger.getAttribute("aria-disabled") === "true");
  const elsewhere = tests.querySelector("#elsewhere");
  elsewhere.focus();
  saving.reject(new Error("Try again later"));
  await until(() => tests.querySelector('[role="alert"]'));
  check(
    document.activeElement === elsewhere,
    "rejected save never steals focus back",
  );
  check(
    trigger.textContent.includes("Blue"),
    "failure retains the saved color",
  );
  await open();
  panel().querySelector('.name-color-option[data-name-color="green"]').click();
  await until(
    () =>
      panel()
        .querySelector('.name-color-option[data-name-color="green"]')
        .getAttribute("aria-pressed") === "true",
  );
  panel().dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  await until(() => !panel());
  check(
    writes.length === 2 && document.activeElement === trigger,
    "Escape cancels without a write",
  );
  await open();
  panel().querySelector('.name-color-option[data-name-color="teal"]').click();
  elsewhere.focus();
  await until(() => !panel());
  check(
    writes.length === 2,
    "outside focus dismisses the preview without saving",
  );
});

await test("color settings keep the root available and expose only authorized room scopes", async () => {
  const { NameColorSettings } =
    await import("../../src/components/settings/NameColorSettings.tsx");
  const baseRoom = {
    name: "allowed",
    id: "allowed-id",
    owner: "bob",
    permissions: ["w:account.color.own"],
    colors: { alice: "blue" },
  };
  let state = {
    ...empty,
    permissions: [],
    rooms: [
      baseRoom,
      {
        ...baseRoom,
        name: "revoked",
        id: "revoked-id",
        permissions: ["r:message.read"],
      },
      {
        ...baseRoom,
        name: "managed",
        id: "managed-id",
        permissions: ["w:account.color.any"],
      },
    ],
  };
  function draw() {
    render(
      h(NameColorSettings, { state, onNameColorChange: async () => {} }),
      tests,
    );
  }
  draw();
  tests.querySelector("#name-color-scope").click();
  await until(() => tests.querySelector("#name-color-scopes:not([inert])"));
  const scopes = [
    ...tests.querySelectorAll(
      '#name-color-scopes:not([inert]) [role="option"]',
    ),
  ];
  check(
    scopes.map((item) => item.textContent.trim()).join(",") ===
      "Everywhere,#allowed,#managed",
    "root needs no grant; revoked rooms stay absent; any enables room control",
  );
  scopes[1].click();
  await until(
    () =>
      tests.querySelector("#name-color-scope").textContent.trim() ===
      "#allowed",
  );
  state = {
    ...state,
    rooms: state.rooms.map((room) => ({ ...room, permissions: [] })),
  };
  draw();
  await until(
    () =>
      tests.querySelector("#name-color-scope").textContent.trim() ===
      "Everywhere",
  );
  check(
    tests.querySelector("#name-color-value").textContent === "Default",
    "revocation falls back to editing the root, without using the room's saved value",
  );
});

await test("login requests saved name colors in its first authenticated snapshot", async () => {
  const key = "commonroom-name-colors";
  const saved = localStorage.getItem(key);
  const login = api.login,
    me = api.me;
  const requests = [];
  try {
    localStorage.setItem(key, "on");
    api.login = async () => ({ ok: true });
    api.me = async (enabled) => {
      requests.push(enabled);
      return { ...empty, colors: { alice: "violet" } };
    };
    let received;
    render(
      h(Login, {
        initialError: "",
        onLogin(account) {
          received = account;
        },
      }),
      tests,
    );
    tests
      .querySelector("form")
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await until(() => received);
    check(
      requests.length === 1 &&
        requests[0] &&
        received.colors.alice === "violet",
      "login supplies colors before rendering Chat",
    );
  } finally {
    api.login = login;
    api.me = me;
    if (saved === null) localStorage.removeItem(key);
    else localStorage.setItem(key, saved);
  }
});

await test("expired attachments preserve filenames across media kinds and themes", async () => {
  const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
  const path = new URL(`api/media/${id}/file`, document.baseURI).pathname;
  const filename = "quarterly-report-原稿.pdf";
  let expired = true;
  let denied = false;
  api.mediaStatus = async () => {
    if (denied) throw new Error("Network unavailable");
    return {
      media_id: id,
      path,
      filename,
      size: 123,
      content_type: "application/pdf",
      expires: Math.ceil(Date.now() / 1000) + 1,
      expired,
    };
  };
  const theme = document.documentElement.dataset.theme;
  try {
    for (const mode of ["light", "dark"]) {
      document.documentElement.dataset.theme = mode;
      for (const markdown of [
        `![image](${path})`,
        `[video](${path} "media:video")`,
        `[audio](${path} "media:audio")`,
        `[document](${path} "media:html")`,
        `[drawing](${path} "media:svg")`,
        `[download](${path})`,
      ]) {
        render(
          h(MarkdownContent, { nodes: markdownNodes(markdown), mentions: [] }),
          tests,
        );
        await until(() => tests.querySelector(".attachment-expired"));
        check(
          tests.textContent.includes(filename),
          "original filename is retained",
        );
        check(
          tests.textContent.includes("Attachment expired"),
          "expiry label is explicit",
        );
        check(
          !tests.querySelector(
            "img:not(.attachment-expired-icon),video,audio,a,button",
          ),
          "expired attachments have no broken media or dead actions",
        );
        const icon = tests.querySelector(".attachment-expired-icon");
        await until(() => icon.complete);
        check(icon.naturalWidth > 0, "approved SVG icon loads successfully");
        const bounds = icon.getBoundingClientRect();
        check(
          bounds.width === bounds.height && bounds.width > 0,
          "icon retains square proportions",
        );
        check(
          getComputedStyle(tests.querySelector(".attachment-expired"))
            .backgroundColor !== "rgba(0, 0, 0, 0)",
          "placeholder uses themed surface",
        );
        render(null, tests);
      }
    }
    expired = false;
    render(
      h(MarkdownContent, {
        nodes: markdownNodes(`[download](${path})`),
        mentions: [],
      }),
      tests,
    );
    await until(() => tests.querySelector("a"));
    expired = true;
    await until(() => tests.querySelector(".attachment-expired"));
    render(null, tests);
    denied = true;
    render(
      h(MarkdownContent, {
        nodes: markdownNodes(`[download](${path})`),
        mentions: [],
      }),
      tests,
    );
    await until(() => tests.querySelector("a"));
    check(
      !tests.querySelector(".attachment-expired"),
      "network/access failures are not labeled expiry",
    );
  } finally {
    if (theme === undefined) delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }
});

document.querySelector("#results").textContent = JSON.stringify(
  results,
  null,
  2,
);
window.browserTestResults = results;
