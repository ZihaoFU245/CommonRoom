import { AppearanceSettings } from "../../src/components/settings/AppearanceSettings.tsx";
import {
  connectPreferences,
  disconnectPreferences,
  receivePreferences,
  preferenceValue,
  flushPreferences,
} from "../../src/features/preferences/account.ts";
import { useChatPreferences } from "../../src/hooks/useChatPreferences.ts";
import { useTheme } from "../../src/hooks/useTheme.ts";
import { useExternalAvatars } from "../../src/hooks/useExternalAvatars.ts";
import { h, render } from "preact";
import { useHistory } from "../../src/hooks/useHistory.ts";
import { useSettingsMotion } from "../../src/hooks/useSettingsMotion.ts";
import { useReadTracking } from "../../src/hooks/useReadTracking.ts";
import { useHistoryNoticeMotion } from "../../src/hooks/useHistoryNoticeMotion.ts";
import { useHistoryPaging } from "../../src/hooks/useHistoryPaging.ts";
import { useCommands } from "../../src/hooks/useCommands.ts";
import { MediaCache } from "../../src/features/conversation/mediaCache.ts";
import { MessageList } from "../../src/components/MessageList.tsx";
import { Avatar } from "../../src/components/Avatar.tsx";
import { Composer } from "../../src/components/Composer.tsx";
import { useComposerResize } from "../../src/hooks/useComposerResize.ts";
import { useRef, useState } from "preact/hooks";
import { useCommandComposer } from "../../src/hooks/useCommandComposer.ts";
import { Chat } from "../../src/components/Chat.tsx";
import { Login } from "../../src/components/Login.tsx";
import { Settings } from "../../src/components/Settings.tsx";
import { MarkdownContent } from "../../src/components/MarkdownContent.tsx";
import { ConsoleOutput } from "../../src/components/ConsoleOutput.tsx";
import { markdownNodes } from "../../src/features/conversation/markdown.ts";
import { api, ApiError } from "../../src/api/client.ts";
import {
  defaultSeeds,
  contrast,
} from "../../src/features/preferences/palette.ts";
import { applyAppearance } from "../../src/features/preferences/appearance.ts";
import "../../src/style.css";

const tests = document.querySelector("#tests");
const results = [];
const testFilter = new URLSearchParams(location.search).get("test");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function check(condition, message) {
  if (!condition) throw new Error(message);
}
async function until(predicate, detail = () => "") {
  const end = Date.now() + 5000;
  while (!predicate()) {
    if (Date.now() > end)
      throw new Error(
        `Timed out waiting for browser state ${predicate.toString()} ${detail()}`,
      );
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
  if (testFilter && !name.includes(testFilter)) return;
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
await test("system metrics sections expand independently while keeping rates visible", async () => {
  for (const command of ["/system metrics", "/sudo /system metrics"]) {
    render(
      h(ConsoleOutput, {
        entry: {
          command,
          result:
            "# System metrics\n\n- History page hit rate: **84.86%**\n\n## History and unread caches\n\n| Metric | Value |\n| --- | ---: |\n| hits | 4752 |\n\n## Snapshot cache\n\n| Metric | Value |\n| --- | ---: |\n| hits | 10 |",
          error: false,
        },
      }),
      tests,
    );
    await until(() => tests.querySelectorAll("details").length === 2);
    const sections = [...tests.querySelectorAll("details")];
    check(
      sections.every((section) => !section.open),
      "Metrics sections start collapsed",
    );
    check(
      tests.querySelector("strong").getBoundingClientRect().height > 0,
      "Hit rate is visible",
    );
    const summary = sections[0].querySelector("summary");
    summary.click();
    check(
      sections[0].open && !sections[1].open,
      "Each section opens independently",
    );
    check(
      sections[0].querySelector("table").getBoundingClientRect().height > 0,
      "Expanded section displays its table",
    );
    summary.click();
    check(!sections[0].open, "Section closes again");
    summary.focus();
    check(
      document.activeElement === summary,
      "Native summary is keyboard focusable",
    );
    render(null, tests);
  }
});

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
          loadSequence() {},
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

await test("keyboard history paging works without scroll overflow and ignores editors", async () => {
  const viewport = document.createElement("section");
  viewport.style.cssText = "height:200px;overflow:auto";
  viewport.tabIndex = 0;
  viewport.append(document.createTextNode("Short page"));
  const editor = document.createElement("textarea");
  viewport.append(editor);
  tests.append(viewport);
  let older = 0,
    newer = 0,
    loading = false;
  const p = probe(() =>
    useHistoryPaging(
      { current: viewport },
      true,
      loading,
      true,
      true,
      () => older++,
      () => newer++,
    ),
  );
  p.update();
  await sleep(50);
  viewport.dispatchEvent(
    new KeyboardEvent("keydown", { key: "PageUp", bubbles: true }),
  );
  viewport.dispatchEvent(
    new KeyboardEvent("keydown", { key: "PageDown", bubbles: true }),
  );
  check(
    older === 1 && newer === 1,
    "keyboard directions load history even on a short page",
  );
  editor.dispatchEvent(
    new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }),
  );
  viewport.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Home", ctrlKey: true, bubbles: true }),
  );
  check(older === 1, "editor and modified keys keep their normal behavior");
  loading = true;
  p.update();
  await sleep(30);
  viewport.dispatchEvent(
    new KeyboardEvent("keydown", { key: "PageUp", bubbles: true }),
  );
  check(older === 1, "keyboard paging cannot duplicate an in-flight request");
  p.close();
  viewport.dispatchEvent(
    new KeyboardEvent("keydown", { key: "End", bubbles: true }),
  );
  check(newer === 1, "unmount removes keyboard listeners");
});

await test("unread navigation resolves a deleted target after paging and within loaded history", async () => {
  api.read = async () => ({ ok: true });
  const viewport = document.createElement("section");
  viewport.style.cssText = "height:200px;overflow:auto";
  const content = document.createElement("div");
  content.style.height = "2000px";
  viewport.append(content);
  tests.append(viewport);
  const jumps = [],
    loads = [];
  const bottom = { current: false };
  const context = {
    selected: "a",
    conversationId: "a-id",
    unread: { first: null, count: 0, oldest: 1, through: 100 },
    messages: [80, 81, 82].map(message),
    historyReady: true,
    historyLoading: false,
    output: [],
    cleared: {},
    resetCleared() {},
    settingsOpen: false,
    menu: false,
    status: "online",
    loadSequence: (sequence) => loads.push(sequence),
    virtualTimeline: {
      current: {
        position: () => ({ sequence: 80, offset: 0 }),
        scrollToSequence: (sequence) => jumps.push(sequence),
      },
    },
    atBottom: bottom,
    historyElement: { current: viewport },
    hasAfter: true,
  };
  const p = probe(() => useReadTracking(context));
  p.update();
  await sleep(30);
  bottom.current = false;
  context.unread = { ...context.unread, first: 90, count: 11 };
  p.update();
  await until(() => p.current.showJumpToUnread);
  p.current.jumpToUnread();
  check(loads[0] === 90, "unloaded destination requests an around page");
  context.historyLoading = true;
  context.messages = [88, 89, 91, 92].map(message);
  context.unread = { ...context.unread, first: 91, count: 10 };
  p.update();
  await sleep(30);
  check(jumps.length === 0, "wait for the history request to complete");
  context.historyLoading = false;
  p.update();
  await until(() => jumps[0] === 91);
  check(!p.current.showJumpToUnread, "fallback consumes the unread jump once");
  context.unread = { ...context.unread, first: null, count: 0 };
  p.update();
  await sleep(30);
  context.unread = { ...context.unread, first: 90, count: 2 };
  p.update();
  await until(() => p.current.showJumpToUnread);
  p.current.jumpToUnread();
  check(
    jumps[1] === 91 && loads.length === 1,
    "loaded gaps resolve immediately without another request",
  );
  p.close();
});

for (const width of [1100, 360]) {
  await test(`unread notice motion leaves scroll geometry stable at ${width}px`, async () => {
    function Notice({ visible, unread = false }) {
      const ref = useHistoryNoticeMotion();
      return h(
        "div",
        {
          style: {
            width: `${width}px`,
            height: "400px",
            display: "flex",
            flexDirection: "column",
          },
        },
        h(
          "div",
          { class: "history-notices", ref },
          visible &&
            h(
              "div",
              { class: "unread-bar", key: "room" },
              h(
                "button",
                null,
                unread ? "Jump to unread ↓" : "Jump to bottom ↓",
              ),
            ),
        ),
        h(
          "section",
          { class: "message-list" },
          Array.from({ length: 30 }, (_, i) =>
            h("div", { key: i, style: { height: "100px" } }, `Message ${i}`),
          ),
        ),
      );
    }
    const paint = () =>
      new Promise((resolve) => requestAnimationFrame(resolve));
    render(h(Notice, { visible: false }), tests);
    await sleep(100);
    const viewport = tests.querySelector(".message-list");
    const originalHeight = viewport.clientHeight;
    viewport.scrollTop = 800;
    render(h(Notice, { visible: true }), tests);
    await until(() => tests.querySelector(".unread-bar"));
    const bar = tests.querySelector(".unread-bar");
    check(
      viewport.clientHeight === originalHeight,
      "floating notice reserves no message space",
    );
    if (!matchMedia("(prefers-reduced-motion: reduce)").matches) {
      await until(() =>
        bar
          .getAnimations()
          .some((animation) => animation.playState === "running"),
      );
      const effect = bar
        .getAnimations()
        .find((animation) => animation.playState === "running").effect;
      check(
        effect.getTiming().duration === 120,
        "notice timing matches shared 120 ms controls",
      );
      check(
        effect
          .getKeyframes()
          .every(
            (frame) =>
              !frame.transform?.includes("scale") &&
              frame.opacity === undefined,
          ),
        "notice only slides, without scaling or fading",
      );
    }
    const heights = [],
      tops = [],
      translations = [];
    for (let i = 0; i < 12; i++) {
      await paint();
      heights.push(viewport.clientHeight);
      tops.push(viewport.scrollTop);
      const transform = getComputedStyle(bar).transform;
      translations.push(
        transform === "none" ? 0 : new DOMMatrixReadOnly(transform).m42,
      );
    }
    check(
      Math.max(...heights) - Math.min(...heights) <= 1,
      `notice motion must not resize the viewport each frame: ${heights}`,
    );
    check(
      Math.max(...tops) - Math.min(...tops) <= 1,
      "notice motion must not shift the reading position",
    );
    const bounds = viewport.getBoundingClientRect();
    check(
      document
        .elementFromPoint(bounds.left + 40, bounds.top + 16)
        ?.closest(".message-list") === viewport,
      "transparent space beside the floating button passes clicks through to messages",
    );
    const jump = bar.querySelector("button");
    const jumpBounds = jump.getBoundingClientRect();
    if (jumpBounds.right < innerWidth)
      check(
        jump.contains(
          document.elementFromPoint(
            jumpBounds.left + jumpBounds.width / 2,
            jumpBounds.top + jumpBounds.height / 2,
          ),
        ),
        "floating button still receives clicks",
      );
    check(
      translations.every(
        (value, index) =>
          value <= 0.5 && (!index || value >= translations[index - 1] - 0.5),
      ),
      `notice enters in one direction with no bounce: ${translations}`,
    );
    check(
      getComputedStyle(viewport).overscrollBehaviorY === "none",
      "chat disables elastic overscroll",
    );
    render(h(Notice, { visible: true, unread: true }), tests);
    await sleep(180);
    check(
      tests.querySelector(".unread-bar") === bar,
      "changing the jump action keeps the same notice node",
    );
    viewport.scrollTo({ top: 1200, behavior: "smooth" });
    render(h(Notice, { visible: false }), tests);
    const removalHeights = [];
    for (let i = 0; i < 12; i++) {
      await paint();
      removalHeights.push(viewport.clientHeight);
    }
    check(
      Math.max(...removalHeights) - Math.min(...removalHeights) <= 1,
      `notice exit must not fight an active scroll: ${removalHeights}`,
    );
  });
}

for (const width of [1100, 360]) {
  await test(`history navigation starts unread, restores rooms and loads automatically at ${width}px`, async () => {
    const savedHash = location.hash;
    const OriginalSocket = window.WebSocket;
    const savedWidth = tests.style.width;
    tests.style.width = `${width}px`;
    history.replaceState(null, "", "#room=a");
    const all = Array.from({ length: 300 }, (_, i) => ({
      ...message(i + 1),
      text: `Message ${i + 1}: ${"你好 🙂 ".repeat((i % 4) + 1)}`,
    }));
    let state = {
      ...empty,
      rooms: ["a", "b"].map((name) => ({
        name,
        id: `${name}-id`,
        owner: "alice",
        members: ["alice"],
        permissions: ["r:message.read", "w:message.create"],
        commands: [],
        messages: all.slice(-20).map((m) => ({ ...m, id: `${name}-${m.id}` })),
      })),
      unread: {
        a: { revision: 1, count: 181, first: 120, oldest: 1, through: 300 },
        b: { revision: 1, count: 0, first: null, oldest: 1, through: 300 },
      },
    };
    const requests = [];
    let socket;
    class TestSocket {
      static OPEN = 1;
      static CLOSED = 3;
      readyState = 1;
      constructor() {
        socket = this;
        queueMicrotask(() => this.onopen?.());
      }
      send() {}
      close() {
        this.readyState = 3;
      }
    }
    window.WebSocket = TestSocket;
    api.me = async () => state;
    api.read = async () => ({ ok: true });
    api.mediaConfig = async () => ({ enabled: false, max_size: 0 });
    api.botCommands = async (id) => ({ conversation_id: id, bots: [] });
    api.history = async (view, _signal, range) => {
      requests.push({ view, ...range });
      let start = Math.max(0, all.length - 100);
      if (range.around) start = Math.max(0, range.around - 50);
      if (range.before) start = Math.max(0, range.before - 101);
      if (range.after) start = range.after;
      const batch = all.slice(start, start + 100);
      return {
        view,
        revision: state.unread[view].revision,
        messages: batch.map((m) => ({ ...m, id: `${view}-${m.id}` })),
        has_before: start > 0,
        has_after: start + batch.length < all.length,
      };
    };
    const button = (label) =>
      [...tests.querySelectorAll("button")].find(
        (node) => node.textContent.trim() === label,
      );
    const rowOffset = (sequence) => {
      const row = tests.querySelector(`[data-sequence="${sequence}"]`);
      const viewport = tests.querySelector(".message-list");
      return row
        ? row.closest("[data-virtual-key]").getBoundingClientRect().top -
            viewport.getBoundingClientRect().top
        : Infinity;
    };
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
        () => Math.abs(rowOffset(120)) < 80,
        () =>
          JSON.stringify({
            actual: rowOffset(120),
            top: tests.querySelector(".message-list")?.scrollTop,
            requests,
          }),
      );
      await sleep(180);
      check(
        !button("Older messages") &&
          !button("Newer messages") &&
          !button("Latest messages"),
        "manual paging controls are removed",
      );
      check(
        !tests.textContent.includes("Loading history…"),
        "automatic paging has no routine loading notice",
      );
      const viewport = tests.querySelector(".message-list");
      viewport.scrollTop += Math.max(300, viewport.clientHeight);
      viewport.dispatchEvent(new Event("scroll"));
      await sleep(150);
      const visibleRow = [...viewport.querySelectorAll("[data-sequence]")].find(
        (node) =>
          node.getBoundingClientRect().bottom >
          viewport.getBoundingClientRect().top,
      );
      const sequence = Number(visibleRow.dataset.sequence);
      const offset = rowOffset(sequence);
      const choose = (name) =>
        [...tests.querySelectorAll(".sidebar button")].find((node) =>
          node.textContent.trim().startsWith(`#${name}`),
        );
      choose("b").click();
      await until(
        () =>
          tests.querySelector(".conversation h1")?.textContent.trim() === "b",
        () =>
          JSON.stringify({
            heading: tests.querySelector(".conversation h1")?.textContent,
            hash: location.hash,
            requests: requests.slice(-5),
          }),
      );
      await sleep(150);
      choose("a").click();
      await until(
        () => Math.abs(rowOffset(sequence) - offset) < 2,
        () =>
          JSON.stringify({
            sequence,
            offset,
            actual: rowOffset(sequence),
            top: viewport.scrollTop,
            requests: requests.slice(-5),
          }),
      );
      // Model the server's read acknowledgement before checking the bottom-only
      // action; the floating control intentionally gives unread priority.
      state = {
        ...state,
        unread: {
          ...state.unread,
          a: { ...state.unread.a, count: 0, first: null },
        },
      };
      socket.onmessage?.({ data: JSON.stringify(state) });
      if (!button("Jump to bottom ↓")) {
        // Load enough older history to be three screens from the loaded edge.
        viewport.scrollTop = 0;
        viewport.dispatchEvent(new Event("scroll"));
        viewport.dispatchEvent(
          new WheelEvent("wheel", { deltaY: -100, bubbles: true }),
        );
      }
      await until(() => button("Jump to bottom ↓"));
      button("Jump to bottom ↓").click();
      await until(
        () => rowOffset(300) < viewport.clientHeight && rowOffset(300) > -100,
      );
      await sleep(450);
      check(!button("Jump to bottom ↓"), "bottom shortcut hides after arrival");
      // Start a new unread boundary while this room is being read above the tail.
      viewport.scrollTop -= viewport.clientHeight * 4;
      viewport.dispatchEvent(new Event("scroll"));
      await sleep(100);
      const previousNotice = tests.querySelector(
        ".history-notices .unread-bar",
      );
      all.push(
        ...[301, 302, 303].map((sequence) => ({
          ...message(sequence),
          text: "New arrival",
        })),
      );
      state = {
        ...state,
        rooms: state.rooms.map((room) =>
          room.name === "a"
            ? {
                ...room,
                messages: all
                  .slice(-20)
                  .map((m) => ({ ...m, id: `a-${m.id}` })),
              }
            : room,
        ),
        unread: {
          ...state.unread,
          a: {
            ...state.unread.a,
            revision: 4,
            count: 3,
            first: 301,
            through: 303,
          },
        },
      };
      socket.onmessage?.({ data: JSON.stringify(state) });
      await until(() => button("Jump to unread ↓"));
      check(
        tests.querySelectorAll(".history-notices button").length === 1,
        "only one floating jump button is shown",
      );
      check(
        !/\d+ unread/u.test(
          tests.querySelector(".history-notices").textContent,
        ),
        "floating navigation has no unread count",
      );
      check(
        tests.querySelector(".history-notices .unread-bar"),
        "new unread notice is inside the AutoAnimate container",
      );
      check(
        !previousNotice ||
          tests.querySelector(".history-notices .unread-bar") ===
            previousNotice,
        "new unread messages update the existing notice without competing exit/entry animations",
      );
      button("Jump to unread ↓").click();
      await until(
        () => rowOffset(301) < viewport.clientHeight && rowOffset(301) > -100,
      );
      await sleep(450);
      check(
        !button("Jump to unread ↓"),
        "the unread shortcut disappears after one use",
      );
      state = {
        ...state,
        unread: {
          ...state.unread,
          a: { ...state.unread.a, first: 302, count: 2 },
        },
      };
      const afterJump = viewport.scrollTop;
      socket.onmessage?.({ data: JSON.stringify(state) });
      await sleep(200);
      check(
        !button("Jump to unread ↓"),
        "advancing read acknowledgements cannot reoffer the jump",
      );
      check(
        Math.abs(viewport.scrollTop - afterJump) < 2,
        "read acknowledgements preserve the reading position",
      );
      // Scrolling up fetches earlier pages without a button.
      const count = requests.length;
      viewport.scrollTop = 1000;
      viewport.dispatchEvent(new Event("scroll"));
      await sleep(100);
      viewport.scrollTop = 0;
      viewport.dispatchEvent(new Event("scroll"));
      await until(() =>
        requests.slice(count).some((request) => request.before),
      );
    } finally {
      render(null, tests);
      tests.style.width = savedWidth;
      window.WebSocket = OriginalSocket;
      history.replaceState(null, "", savedHash || location.pathname);
    }
  });
}

await test("partial palette input stays local and cannot block other settings from syncing", async () => {
  let state = { values: {}, sync: { instance: "preferences", revision: 0 } };
  await connectPreferences("browser-palette-test", async (patch) => {
    state = {
      values: { ...state.values, ...patch },
      sync: { instance: "preferences", revision: state.sync.revision + 1 },
    };
    return state;
  });
  function AppearanceProbe() {
    const appearance = useTheme();
    const { fonts, setFonts } = useChatPreferences();
    return h(AppearanceSettings, {
      ...appearance,
      fonts,
      onChange: setFonts,
      onThemeChange: appearance.setTheme,
      onSeedsChange: appearance.setSeeds,
    });
  }
  try {
    render(h(AppearanceProbe), tests);
    tests.querySelector('[value="custom"]').click();
    await until(() => tests.querySelector("#palette-base"));
    const original = state.values["palette.base"];
    const input = tests.querySelector("#palette-base");
    input.focus();
    input.value = "#12";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    const font = tests.querySelector("#chat-font-size");
    font.value = "22";
    font.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => tests.querySelector("#chat-font-size").value === "22");
    await flushPreferences();
    check(
      state.values["palette.base"] === original,
      "incomplete color never reaches cloud settings",
    );
    check(
      state.values["font.chat"] === "22",
      "incomplete color cannot reject unrelated font changes",
    );
    input.value = "#123abc";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPreferences();
    check(state.values["palette.base"] === "#123abc", "valid full color syncs");
    input.value = "#abc";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await sleep(30);
    input.blur();
    await flushPreferences();
    check(
      state.values["palette.base"] === "#aabbcc",
      "short color expands before syncing on blur",
    );
  } finally {
    render(null, tests);
    disconnectPreferences();
    applyAppearance("light", false, defaultSeeds);
    document.documentElement.style.removeProperty("--chat-font-size");
    document.documentElement.style.removeProperty("--ui-font-size");
  }
});

await test("account preferences update rendered controls and appearance across devices", async () => {
  let state = { values: {}, sync: { instance: "preferences", revision: 0 } };
  await connectPreferences("browser-preferences-test", async (patch) => {
    state = {
      values: { ...state.values, ...patch },
      sync: { instance: "preferences", revision: state.sync.revision + 1 },
    };
    return state;
  });
  const view = probe(() => ({
    chat: useChatPreferences(),
    appearance: useTheme(),
    external: useExternalAvatars(),
  }));
  try {
    view.update();
    await sleep(80);
    state = {
      values: {
        ...state.values,
        theme: "dark",
        "font.chat": "23",
        name_colors: "true",
        message_bars: "true",
        allow_html: "true",
        external_avatars: "true",
      },
      sync: { instance: "preferences", revision: state.sync.revision + 1 },
    };
    receivePreferences(state);
    await until(
      () =>
        view.current.chat.fonts.chat === 23 &&
        view.current.chat.showNameColors &&
        view.current.chat.messageBars &&
        view.current.chat.allowHtml &&
        view.current.external[0] &&
        view.current.appearance.theme === "dark",
    );
    await until(
      () =>
        document.documentElement.dataset.theme === "dark" &&
        document.documentElement.style.getPropertyValue("--chat-font-size") ===
          "23px",
    );
    view.current.chat.setMessageBars(false);
    await flushPreferences();
    check(
      state.values.message_bars === "false",
      "local control patches cloud settings",
    );
    check(
      state.values.theme === "dark" && state.values.allow_html === "true",
      "patch preserves settings received from the other device",
    );
  } finally {
    view.close();
    disconnectPreferences();
    applyAppearance("light", false, defaultSeeds);
    document.documentElement.style.removeProperty("--chat-font-size");
    document.documentElement.style.removeProperty("--ui-font-size");
  }
});

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
  let preferenceState = {
    values: {},
    sync: { instance: "preferences", revision: 0 },
  };
  await connectPreferences("browser-color-test", async (patch) => {
    preferenceState = {
      values: { ...preferenceState.values, ...patch },
      sync: {
        instance: "preferences",
        revision: preferenceState.sync.revision + 1,
      },
    };
    return preferenceState;
  });
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
    await until(() => socket && requests.length > 0 && author("alice"));
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
        preferenceValue("message_bar_opacity", "") === String(value),
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
    const viewport = tests.querySelector(".message-list");
    viewport.scrollTop = 100;
    viewport.dispatchEvent(new Event("scroll"));
    await sleep(30);
    viewport.scrollTop = 0;
    viewport.dispatchEvent(new Event("scroll"));
    viewport.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -100, bubbles: true }),
    );
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
    disconnectPreferences();
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
    document.activeElement === action("Saving…"),
    "saving keeps Apply focused and displays its pending state",
  );
  trigger.click();
  check(panel(), "saving keeps the draft open and blocks toggling the picker");
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

for (const width of [1000, 320]) {
  await test(`legacy reply quotes reveal originals outside the virtual window (${width}px)`, async () => {
    const start = Math.floor(Date.now() / 60000) * 60;
    const entries = Array.from({ length: 60 }, (_, index) => {
      const item = {
        ...message(index + 1),
        from: "alice",
        time: start + index,
        text: `message ${index + 1} `.repeat(6),
      };
      return {
        kind: "message",
        key: item.id,
        order: item.sequence,
        message: item,
      };
    });
    entries.at(-1).message.reply = {
      id: "m-1",
      from: "alice",
      text: "message 1",
    };
    const history = { current: null },
      timeline = { current: null },
      end = { current: null },
      bottom = { current: true };
    render(
      h(
        "div",
        {
          style: {
            display: "flex",
            flexDirection: "column",
            height: "500px",
            width: `${width}px`,
          },
        },
        h(MessageList, {
          motionScope: "quote-jump-test",
          revision: 1,
          historyReady: true,
          entries,
          username: "alice",
          pending: false,
          debug: false,
          allowHtml: false,
          latex: false,
          permissions: ["r:message.read", "w:message.create"],
          commands: [{ name: "/reply" }],
          unreadBoundary: null,
          atBottom: bottom,
          historyElement: history,
          virtualTimeline: timeline,
          end,
          onReact() {},
          onReply() {},
          onDelete() {},
          loadSequence() {},
          onEdit: async () => true,
          onScroll() {},
        }),
      ),
      tests,
    );
    await until(() => tests.querySelector("#message-m-1"));
    const viewport = tests.querySelector(".message-list");
    viewport.scrollTop = viewport.scrollHeight;
    viewport.dispatchEvent(new Event("scroll"));
    await until(() => tests.querySelector("#message-m-60"));
    await until(() => tests.querySelector("#message-m-1") === null);
    check(
      tests.querySelector("#message-m-60"),
      "last message is rendered at bottom",
    );
    tests.querySelector(".reply-quote").click();
    await until(() => tests.querySelector("#message-m-1"));
    await until(() => !timeline.current?.isScrolling?.());
    const element = tests.querySelector("#message-m-1");
    const target = element.getBoundingClientRect();
    const bounds = viewport.getBoundingClientRect();
    check(
      target.top >= bounds.top && target.bottom <= bounds.bottom,
      "original message is scrolled into view",
    );
    const probe = document.createElement("div");
    probe.style.background = "var(--wash)";
    tests.append(probe);
    const wash = getComputedStyle(probe).backgroundColor;
    probe.remove();
    check(
      getComputedStyle(element).backgroundColor === wash,
      "original message flashes the hover wash",
    );
    await new Promise((resolve) => setTimeout(resolve, 1400));
    check(
      getComputedStyle(element).backgroundColor === "rgba(0, 0, 0, 0)",
      "the highlight fades after the jump",
    );
  });

  await test(`reply quotes page to originals outside the loaded history (${width}px)`, async () => {
    const start = Math.floor(Date.now() / 60000) * 60;
    const page = (first, last) =>
      Array.from({ length: last - first + 1 }, (_, index) => {
        const item = {
          ...message(first + index),
          from: "alice",
          time: start + first + index,
          text: `message ${first + index} `.repeat(6),
        };
        return {
          kind: "message",
          key: item.id,
          order: item.sequence,
          message: item,
        };
      });
    const initial = page(101, 160);
    initial.at(-1).message.reply = {
      id: "m-1",
      from: "alice",
      text: "message 1",
      sequence: 1,
    };
    const loaded = page(1, 60);
    const history = { current: null },
      timeline = { current: null },
      end = { current: null },
      bottom = { current: true };
    const requested = [];
    let paged = false;
    const mount = (entries) =>
      render(
        h(
          "div",
          {
            style: {
              display: "flex",
              flexDirection: "column",
              height: "500px",
              width: `${width}px`,
            },
          },
          h(MessageList, {
            motionScope: "quote-page-test",
            revision: 1,
            historyReady: true,
            entries,
            username: "alice",
            pending: false,
            debug: false,
            allowHtml: false,
            latex: false,
            permissions: ["r:message.read", "w:message.create"],
            commands: [{ name: "/reply" }],
            unreadBoundary: null,
            atBottom: bottom,
            historyElement: history,
            virtualTimeline: timeline,
            end,
            onReact() {},
            onReply() {},
            onDelete() {},
            loadSequence(sequence) {
              requested.push(sequence);
              setTimeout(() => {
                paged = true;
                mount(loaded);
              }, 0);
            },
            onEdit: async () => true,
            onScroll() {},
          }),
        ),
        tests,
      );
    mount(initial);
    await until(() => tests.querySelector("#message-m-101"));
    const viewport = tests.querySelector(".message-list");
    viewport.scrollTop = viewport.scrollHeight;
    viewport.dispatchEvent(new Event("scroll"));
    await until(() => tests.querySelector("#message-m-160"));
    tests.querySelector(".reply-quote").click();
    await until(() => paged);
    await until(() => tests.querySelector("#message-m-1"));
    check(
      requested.length === 1 && requested[0] === 1,
      "the missing original is requested by sequence",
    );
    await until(() => !timeline.current?.isScrolling?.());
    const element = tests.querySelector("#message-m-1");
    const target = element.getBoundingClientRect();
    const bounds = viewport.getBoundingClientRect();
    check(
      target.top >= bounds.top && target.bottom <= bounds.bottom,
      "the paged-to original is scrolled into view",
    );
  });
}

await test("mounted reply jumps release bottom following and clean up motion", async () => {
  const original = window.matchMedia;
  const listeners = new Set();
  let reduced = false;
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
  const entries = [1, 2, 3].map((sequence) => ({
    kind: "message",
    key: `m-${sequence}`,
    order: sequence,
    message: message(sequence),
  }));
  entries[2].message.reply = { id: "m-1", from: "alice", text: "hello" };
  const bottom = { current: true };
  let scope = "mounted-reply";
  const mount = () =>
    render(
      h(
        "div",
        {
          style: { height: "300px", display: "flex", flexDirection: "column" },
        },
        h(MessageList, {
          motionScope: scope,
          revision: 1,
          historyReady: true,
          entries,
          username: "alice",
          pending: false,
          debug: false,
          allowHtml: false,
          latex: false,
          permissions: [],
          commands: [],
          unreadBoundary: null,
          atBottom: bottom,
          historyElement: { current: null },
          virtualTimeline: { current: null },
          end: { current: null },
          onReact() {},
          onReply() {},
          onDelete() {},
          onScroll() {},
          loadSequence() {
            throw new Error("loaded originals must not fetch history");
          },
          onEdit: async () => true,
        }),
      ),
      tests,
    );
  try {
    mount();
    await until(() => tests.querySelector(".reply-quote"));
    const target = tests.querySelector("#message-m-1");
    tests.querySelector(".reply-quote").click();
    check(
      bottom.current === false,
      "mounted jumps release bottom intent synchronously",
    );
    await until(() => target.getAnimations().length > 0);
    const animation = target.getAnimations()[0];
    check(animation, "a mounted target is highlighted");
    reduced = true;
    for (const listener of listeners) listener();
    check(
      animation.playState === "idle",
      "reduced motion cancels the highlight",
    );
    tests.querySelector(".reply-quote").click();
    check(
      target.getAnimations().length === 0,
      "reduced motion skips new highlights",
    );
    reduced = false;
    tests.querySelector(".reply-quote").click();
    await until(() => target.getAnimations().length > 0);
    const beforeSwitch = target.getAnimations()[0];
    scope = "another-conversation";
    mount();
    await until(() => beforeSwitch.playState === "idle");
    tests.querySelector(".reply-quote").click();
    await until(() => target.getAnimations().length > 0);
    const beforeUnmount = target.getAnimations()[0];
    render(null, tests);
    check(beforeUnmount.playState === "idle", "unmount cancels the highlight");
    check(listeners.size === 0, "unmount removes preference listeners");
  } finally {
    render(null, tests);
    window.matchMedia = original;
  }
});

await test("renaming an account reveals its cached avatar without a refresh", async () => {
  const source = `data:image/svg+xml,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="28" height="28"><rect width="28" height="28" fill="orange"/></svg>')}`;
  const draw = (name) =>
    render(h(Avatar, { name, url: undefined, preview: source }), tests);
  draw("alice");
  await until(() => tests.querySelector("img:not([hidden])")?.complete);
  draw("alice-renamed");
  await until(() => tests.querySelector("img:not([hidden])")?.complete);
  check(
    tests.querySelectorAll("img").length === 1,
    "rename leaves one visible avatar",
  );
});

await test("bot credential controls show local dates and insert IDs at wide and compact widths", async () => {
  const credential = {
    id: "11111111-2222-4333-8444-555555555555",
    bot: "helper",
    label: "production",
    kind: "worker",
  };
  const command = {
    name: "/bot",
    usage: "/bot token rm <credential id>",
    description: "Manage bots",
  };
  const pluginCommands = [];
  for (const width of [1200, 320]) {
    const input = { current: null };
    const credentials = [
      credential,
      { ...credential, id: "other-credential", label: "secondary" },
    ];
    function Probe({ available = credentials }) {
      const composer = useCommandComposer({
        botCommands: pluginCommands,
        selected: "@command",
        direct: false,
        peer: null,
        state: { ...empty, bots: ["helper"], bot_credentials: available },
        room: undefined,
        input,
        pending: false,
        aliases: {},
        serverCommands: [command],
        availableCommands: [command],
        send(event) {
          event.preventDefault();
        },
      });
      return h(
        "section",
        { style: { width: `${width}px`, maxWidth: "100%" } },
        [
          h(ConsoleOutput, {
            entry: {
              command: "/bot token ls helper",
              result: JSON.stringify([
                { ...credential, created: 1793283033, expires: null },
              ]),
              error: false,
            },
          }),
          h(Composer, {
            minimumComposerHeight: 40,
            actualComposerHeight: 40,
            hints: composer.hints,
            activeHint: composer.activeHint,
            replyTarget: null,
            input,
            consoleView: true,
            draft: composer.draft,
            pending: false,
            canWrite: true,
            startResize() {},
            moveResize() {},
            endResize() {},
            resizeComposer() {},
            acceptHint: composer.acceptHint,
            cancelReply() {},
            send(event) {
              event.preventDefault();
            },
            edit: composer.edit,
            keydown: composer.keydown,
            selectCaret: composer.selectCaret,
          }),
        ],
      );
    }
    render(h(Probe), tests);
    await until(() => tests.textContent.includes("No expiry"));
    const expectedDate = new Intl.DateTimeFormat(undefined, {
      dateStyle: "full",
      timeStyle: "long",
    }).format(new Date(1793283033000));
    check(
      tests.textContent.includes(expectedDate),
      "credential creation uses a readable local date",
    );
    check(
      !tests.textContent.includes("1793283033"),
      "raw epoch seconds are not displayed",
    );
    const textarea = tests.querySelector("textarea");
    textarea.focus();
    textarea.value = "/bot token rm ";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => tests.querySelector('[role="option"] button'));
    const hint = tests.querySelector('[role="option"] button');
    check(
      hint.textContent === "helper · production",
      "suggestion identifies the bot and label",
    );
    check(
      hint.title.includes("worker") && hint.title.includes(credential.id),
      "suggestion describes type and ID",
    );
    hint.click();
    await until(() => textarea.value === `/bot token rm ${credential.id} `);
    textarea.value = "/bot token rm production";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => tests.querySelector('[role="option"] button'));
    textarea.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        bubbles: true,
        cancelable: true,
      }),
    );
    await until(() => textarea.value === `/bot token rm ${credential.id} `);
    check(
      tests.querySelector("section").scrollWidth <=
        tests.querySelector("section").clientWidth + 2,
      "credential output does not overflow its container",
    );
    check(
      tests.querySelector("section").getBoundingClientRect().width <= width + 1,
      "controls fit their responsive container",
    );
    textarea.value = "/bot token rm ";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => tests.querySelectorAll('[role="option"]').length === 2);
    textarea.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        bubbles: true,
        cancelable: true,
      }),
    );
    await until(() => textarea.value === `/bot token rm ${credential.id} `);
    render(h(Probe, { available: [] }), tests);
    await until(() => !tests.querySelector('[role="option"]'));
    render(null, tests);
  }
});

await test("Safari IME confirmation never sends or selects composer hints", async () => {
  const input = { current: null };
  let sent = 0;
  const send = (event) => {
    event.preventDefault();
    sent++;
  };
  function Probe() {
    const composer = useCommandComposer({
      botCommands: [],
      selected: "team",
      direct: false,
      peer: null,
      state: empty,
      room: undefined,
      input,
      pending: false,
      aliases: {},
      serverCommands: [],
      availableCommands: [],
      send,
    });
    return h(Composer, {
      minimumComposerHeight: 40,
      actualComposerHeight: 40,
      hints: composer.hints,
      activeHint: composer.activeHint,
      replyTarget: null,
      input,
      consoleView: false,
      draft: composer.draft,
      pending: false,
      canWrite: true,
      startResize() {},
      moveResize() {},
      endResize() {},
      resizeComposer() {},
      acceptHint: composer.acceptHint,
      cancelReply() {},
      send,
      edit: composer.edit,
      keydown: composer.keydown,
      selectCaret: composer.selectCaret,
    });
  }
  render(h(Probe), tests);
  const textarea = tests.querySelector("textarea");
  textarea.value = "中文";
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
  await sleep(10);
  const key = (init = {}) => {
    const event = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
      ...init,
    });
    textarea.dispatchEvent(event);
    return event;
  };
  check(
    !key({ isComposing: true }).defaultPrevented && sent === 0,
    "native composing Enter stays with the IME",
  );
  check(
    !key({ keyCode: 229 }).defaultPrevented && sent === 0,
    "Safari's IME key code is ignored even without isComposing",
  );
  textarea.dispatchEvent(
    new CompositionEvent("compositionstart", { bubbles: true }),
  );
  check(
    !key().defaultPrevented && sent === 0,
    "composition events guard unmarked candidate Enter",
  );
  textarea.dispatchEvent(
    new CompositionEvent("compositionend", { bubbles: true }),
  );
  check(
    !key().defaultPrevented && sent === 0,
    "Safari's late Enter after compositionend only commits the candidate",
  );
  textarea.dispatchEvent(
    new CompositionEvent("compositionstart", { bubbles: true }),
  );
  await sleep(10);
  check(
    !key({ key: "Tab" }).defaultPrevented && sent === 0,
    "a new composition cancels the previous reset timer",
  );
  textarea.dispatchEvent(
    new CompositionEvent("compositionend", { bubbles: true }),
  );
  await sleep(10);
  check(
    !key({ shiftKey: true }).defaultPrevented && sent === 0,
    "Shift+Enter still inserts a newline after composition",
  );
  key();
  check(sent === 1, "a deliberate later Enter sends normally");
  textarea.dispatchEvent(
    new CompositionEvent("compositionstart", { bubbles: true }),
  );
  textarea.focus();
  textarea.blur();
  key();
  check(sent === 2, "leaving the input resets an interrupted composition");
});

await test("name-color menus survive mouse clicks without native button focus", async () => {
  const { NameColorSettings } =
    await import("../../src/components/settings/NameColorSettings.tsx");
  render(
    h(
      "section",
      { class: "settings-content", tabIndex: 0 },
      h(NameColorSettings, {
        state: { ...empty, colors: { alice: "red" } },
        onNameColorChange: async () => {},
      }),
    ),
    tests,
  );
  const section = tests.querySelector("section");
  const trigger = tests.querySelector(
    ".name-color-toggle:not(.name-scope-toggle)",
  );
  const panel = () => tests.querySelector("#name-color-options:not([inert])");
  function mouseClick(button) {
    button.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    const down = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
      button: 0,
    });
    // macOS Safari does not focus clicked buttons. Model its default focus
    // fallback to the surrounding focusable dialog content instead.
    if (button.dispatchEvent(down)) section.focus();
    button.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    button.click();
  }
  mouseClick(trigger);
  await until(panel);
  mouseClick(
    panel().querySelector('.name-color-option[data-name-color="blue"]'),
  );
  await sleep(20);
  check(
    panel()?.querySelector(".name-color-preview strong").dataset.nameColor ===
      "blue",
    "swatch mouse activation keeps the picker open and previews the chosen color",
  );
  const cancel = [...panel().querySelectorAll("button")].find(
    (button) => button.textContent === "Cancel",
  );
  mouseClick(cancel);
  await until(() => !panel());
  check(
    document.activeElement === trigger,
    "Cancel restores focus after a pointer click",
  );
  mouseClick(trigger);
  await until(panel);
  section.focus();
  await until(() => !panel());
  check(!panel(), "keyboard focus leaving the menu still dismisses it");
});

for (const failure of [
  { status: 403, message: "Permission to change this color was revoked." },
  {
    status: 503,
    message: "Storage unavailable.",
    display: "Couldn't save your changes right now. Please try again.",
  },
  {
    status: 502,
    message: "Server request failed (502).",
    plain: true,
    display: "Couldn't save your changes right now. Please try again.",
  },
  {
    message: "Network connection lost.",
    display:
      "Couldn't connect to the server. Check your connection and try again.",
  },
]) {
  await test(`color save failures stay visible and preserve retry drafts (${failure.status ?? "network"})`, async () => {
    const { NameColorSettings } =
      await import("../../src/components/settings/NameColorSettings.tsx");
    const originalFetch = window.fetch;
    let fail = true,
      requests = 0;
    window.fetch = async () => {
      requests++;
      if (!fail) return new Response(JSON.stringify({ ok: true }));
      if (!failure.status) throw new TypeError(failure.message);
      return new Response(
        failure.plain
          ? "Bad gateway"
          : JSON.stringify({ error: failure.message }),
        { status: failure.status },
      );
    };
    try {
      render(
        h(NameColorSettings, {
          state: { ...empty, colors: { alice: "red" } },
          onNameColorChange: api.color,
        }),
        tests,
      );
      tests.querySelector(".name-color-toggle:not(.name-scope-toggle)").click();
      const panel = () =>
        tests.querySelector("#name-color-options:not([inert])");
      await until(panel);
      panel().style.maxHeight = "160px";
      panel()
        .querySelector('[data-name-color="blue"].name-color-option')
        .click();
      await until(
        () =>
          panel().querySelector('.name-color-option[aria-pressed="true"]')
            .dataset.nameColor === "blue",
      );
      panel().querySelector(".primary").click();
      panel().querySelector(".primary").click();
      await until(() => tests.querySelector('[role="alert"]'));
      check(
        requests === 1,
        "repeated Apply clicks cannot duplicate an in-flight save",
      );
      check(
        panel()?.querySelector('[role="alert"]').textContent ===
          (failure.display ?? failure.message),
        "the server or network error is visible beside Apply",
      );
      const alert = panel()
        .querySelector('[role="alert"]')
        .getBoundingClientRect();
      const bounds = panel().getBoundingClientRect();
      check(
        alert.top >= bounds.top && alert.bottom <= bounds.bottom,
        "the error scrolls into view in a constrained picker",
      );
      check(
        panel().querySelector('.name-color-option[aria-pressed="true"]').dataset
          .nameColor === "blue",
        "failed color choice stays selected for retry",
      );
      check(
        !tests.querySelector('[role="status"]'),
        "failure never reports success",
      );
      fail = false;
      panel().querySelector(".primary").click();
      await until(() => !panel());
      check(requests === 2, "retry sends exactly one additional request");
      check(
        tests.querySelector('[role="status"]').textContent ===
          "Name color saved.",
        "success is reported only after acknowledgement",
      );
    } finally {
      window.fetch = originalFetch;
    }
  });
}

await test("account save errors preserve drafts and remain visible until a successful retry", async () => {
  const { AccountEditor } =
    await import("../../src/components/settings/AccountEditor.tsx");
  const state = {
    ...empty,
    account_access: {
      ...empty.account_access,
      commands: [{ name: "/user" }],
      permissions: ["w:account.rename.own", "w:account.password.own"],
    },
  };
  let response = { kind: "error", id: 1, text: "Username already exists." };
  let network = false;
  const props = {
    state,
    pending: false,
    section: "account",
    onAccountCommand: async () => {
      if (network)
        throw new Error("Disconnected. Wait for the connection to recover.");
      return response;
    },
  };
  render(h(AccountEditor, props), tests);
  tests.querySelector(".account-setting-action").click();
  await until(() => tests.querySelector("#account-name"));
  const input = tests.querySelector("#account-name");
  input.value = "new-name";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await sleep(10);
  const submit = () =>
    tests
      .querySelector("form")
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  submit();
  await until(
    () => tests.querySelector('[role="alert"]')?.textContent === response.text,
  );
  check(input.value === "new-name", "server rejection retains the name draft");
  response = {
    kind: "error",
    id: 4,
    text: "Storage unavailable; rename was not applied.",
  };
  submit();
  await until(
    () =>
      tests.querySelector('[role="alert"]')?.textContent ===
      "Couldn't save your changes right now. Please try again.",
  );
  network = true;
  submit();
  await until(() =>
    tests.querySelector('[role="alert"]')?.textContent.includes("Disconnected"),
  );
  check(input.value === "new-name", "network rejection retains the name draft");
  network = false;
  response = { kind: "notice", id: 2, text: "Username changed." };
  submit();
  await until(() => !tests.querySelector("#account-name"));
  check(
    tests.querySelector('[role="status"]').textContent === response.text,
    "successful retry closes the editor and reports confirmation",
  );
  render(null, tests);
  response = { kind: "error", id: 3, text: "Current password is incorrect." };
  render(h(AccountEditor, { ...props, section: "security" }), tests);
  tests.querySelector(".account-setting-action").click();
  await until(() => tests.querySelector("#account-old-password"));
  for (const [id, value] of [
    ["account-old-password", "old"],
    ["account-new-password", "new"],
    ["account-confirm-password", "new"],
  ]) {
    const field = tests.querySelector(`#${id}`);
    field.value = value;
    field.dispatchEvent(new Event("input", { bubbles: true }));
  }
  await sleep(10);
  submit();
  await until(
    () => tests.querySelector('[role="alert"]')?.textContent === response.text,
  );
  check(
    tests.querySelector("#account-new-password").value === "new",
    "rejected password saves preserve inputs for correction",
  );
});

await test("avatar upload failures retain the crop and expose a retryable error", async () => {
  const { AvatarSettings } =
    await import("../../src/components/AvatarSettings.tsx");
  let attempts = 0;
  api.allocateAvatar = async () => {
    attempts++;
    throw new ApiError(503, "Avatar storage unavailable.");
  };
  api.abandonMedia = async () => ({ ok: true });
  render(h(AvatarSettings, { state: empty }), tests);
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 32;
  const blob = await new Promise((resolve) =>
    canvas.toBlob(resolve, "image/png"),
  );
  const transfer = new DataTransfer();
  transfer.items.add(new File([blob], "avatar.png", { type: "image/png" }));
  const file = tests.querySelector('input[type="file"]');
  file.files = transfer.files;
  file.dispatchEvent(new Event("change", { bubbles: true }));
  const confirm = () =>
    [...tests.querySelectorAll("button")].find(
      (button) => button.textContent.trim() === "Confirm and upload",
    );
  await until(() => confirm() && !confirm().disabled);
  const crop = tests.querySelector(".avatar-crop canvas");
  confirm().click();
  await until(
    () =>
      tests.querySelector('[role="alert"]')?.textContent ===
        "Couldn't save your changes right now. Please try again." &&
      !confirm()?.disabled,
  );
  check(
    tests.querySelector(".avatar-crop canvas") === crop,
    "the selected image and crop remain available after failure",
  );
  check(
    !tests.textContent.includes("Avatar updated."),
    "failed uploads never claim success",
  );
  confirm().click();
  await until(() => attempts === 4 && !confirm()?.disabled);
  check(
    tests.querySelector(".avatar-crop canvas") === crop,
    "the same crop can be retried without selecting a file again",
  );
});

await test("new-message announcements exclude initial history, backfill and virtual remounts", async () => {
  const bottom = { current: true },
    history = { current: null },
    timeline = { current: null },
    end = { current: null };
  const entry = (sequence) => ({
    kind: "message",
    key: `m-${sequence}`,
    order: sequence,
    message: message(sequence),
  });
  let entries = [entry(2)];
  const draw = () =>
    render(
      h(
        "div",
        {
          style: { height: "300px", display: "flex", flexDirection: "column" },
        },
        h(MessageList, {
          motionScope: "announcements",
          revision: entries.length,
          historyReady: true,
          entries,
          username: "alice",
          pending: false,
          debug: false,
          allowHtml: false,
          latex: false,
          permissions: [],
          commands: [],
          unreadBoundary: null,
          atBottom: bottom,
          historyElement: history,
          virtualTimeline: timeline,
          end,
          onReact() {},
          onReply() {},
          onDelete() {},
          onScroll() {},
          loadSequence() {},
          onEdit: async () => true,
        }),
      ),
      tests,
    );
  draw();
  await sleep(150);
  const log = () => tests.querySelector('[role="log"]');
  check(log() && !log().textContent.trim(), "initial history is silent");
  check(
    !history.current.hasAttribute("aria-live"),
    "virtual history is not a live region",
  );
  entries = [
    ...entries,
    { ...entry(3), message: { ...message(3), text: "a new arrival" } },
  ];
  draw();
  await until(() => log().textContent.includes("a new arrival"));
  entries = [entry(1), ...entries];
  draw();
  await sleep(150);
  check(
    !log().textContent.trim(),
    "backfill does not replay history or the previous arrival",
  );
  history.current.dispatchEvent(new Event("scroll"));
  await sleep(150);
  check(!log().textContent.trim(), "virtual remounts do not announce history");
});

await test("external message images require URL-specific consent", async () => {
  const first = "https://tracker.invalid/first.png";
  const second = "https://tracker.invalid/second.png";
  const draw = (src) =>
    render(
      h(MarkdownContent, {
        nodes: markdownNodes(`![Picture](${src})`),
        mentions: [],
      }),
      tests,
    );
  draw(first);
  check(
    !tests.querySelector("img"),
    "remote image has no network-capable element before consent",
  );
  const button = tests.querySelector("button");
  check(
    button?.textContent.includes("tracker.invalid"),
    "consent identifies the destination",
  );
  button.click();
  await until(() => tests.querySelector("img"));
  check(tests.querySelector("img").src === first, "approved image loads");
  check(
    tests.querySelector("img").referrerPolicy === "no-referrer",
    "image requests omit page referrers",
  );
  draw(second);
  check(!tests.querySelector("img"), "an edited URL requires fresh consent");
  draw("data:image/png;base64,iVBORw0KGgo=");
  check(tests.querySelector("img"), "embedded raster images remain immediate");
});

for (const native of [false, true]) {
  await test(`composer grows and shrinks with ${native ? "native" : "fallback"} sizing`, async () => {
    const supports = CSS.supports;
    if (native && !supports.call(CSS, "field-sizing", "content")) return;
    CSS.supports = (...args) =>
      args[0] === "field-sizing" ? native : supports.apply(CSS, args);
    let edit;
    function Editor() {
      const input = useRef(null);
      const [draft, setDraft] = useState("");
      edit = setDraft;
      const { actualComposerHeight } = useComposerResize(14, input, draft);
      return h("textarea", {
        ref: input,
        value: draft,
        style: { width: "180px", height: `${actualComposerHeight}px` },
      });
    }
    try {
      render(h(Editor), tests);
      const element = tests.querySelector("textarea");
      await sleep(80);
      const minimum = element.getBoundingClientRect().height;
      edit("line\n".repeat(8));
      await until(() => element.getBoundingClientRect().height > minimum + 20);
      edit("");
      await until(() => element.getBoundingClientRect().height <= minimum + 2);
      check(
        native
          ? !document.querySelector('[name="message-size-mirror"]')
          : document.querySelector('[name="message-size-mirror"]'),
        "only fallback sizing uses a mirror",
      );
    } finally {
      render(null, tests);
      CSS.supports = supports;
    }
  });
}

await test("room selection preserves fragments and restores the default view with Back", async () => {
  const Socket = window.WebSocket;
  const url = location.href;
  const sidebar = localStorage.getItem("commonroom-sidebar-open");
  window.WebSocket = class {
    static OPEN = 1;
    static CLOSED = 3;
    readyState = 1;
    close() {
      this.readyState = 3;
    }
  };
  localStorage.setItem("commonroom-sidebar-open", "true");
  history.replaceState(null, "", "#tab=files");
  const state = {
    ...empty,
    rooms: ["alpha", "beta"].map((name) => ({
      id: name,
      name,
      owner: "alice",
      permissions: [],
      commands: [],
      members: ["alice"],
      messages: [],
    })),
    unread: Object.fromEntries(
      ["alpha", "beta"].map((name) => [
        name,
        { count: 0, first: null, through: 0, oldest: 0, revision: 0 },
      ]),
    ),
  };
  try {
    render(
      h(Chat, {
        initial: state,
        onLogout() {},
        appearance: {
          theme: "light",
          setTheme() {},
          seeds: defaultSeeds,
          setSeeds() {},
        },
      }),
      tests,
    );
    await until(
      () => tests.querySelector(".room-title h1")?.textContent === "alpha",
    );
    await sleep(100);
    check(
      location.hash === "#tab=files",
      "mount does not rewrite the default URL",
    );
    const beta = [...tests.querySelectorAll(".room-link")].find((button) =>
      button.textContent.includes("beta"),
    );
    check(beta, "second room is available");
    beta.click();
    await until(
      () => tests.querySelector(".room-title h1")?.textContent === "beta",
    );
    check(
      new URLSearchParams(location.hash.slice(1)).get("tab") === "files",
      "room changes preserve other fragment parameters",
    );
    history.back();
    await until(
      () => tests.querySelector(".room-title h1")?.textContent === "alpha",
    );
    check(
      location.hash === "#tab=files",
      "Back restores the default view and original fragment",
    );
    history.forward();
    await until(
      () => tests.querySelector(".room-title h1")?.textContent === "beta",
    );
  } finally {
    render(null, tests);
    window.WebSocket = Socket;
    history.replaceState(null, "", url);
    if (sidebar === null) localStorage.removeItem("commonroom-sidebar-open");
    else localStorage.setItem("commonroom-sidebar-open", sidebar);
  }
});

document.querySelector("#results").textContent = JSON.stringify(
  results,
  null,
  2,
);
window.browserTestResults = results;
