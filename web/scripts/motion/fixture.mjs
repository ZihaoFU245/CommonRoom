import { h, render } from "preact";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { MessageList } from "../../src/components/MessageList.tsx";
import "../../src/style.css";

const base = new Date(2026, 9, 12, 12).getTime() / 1000;
const message = (
  n,
  from = "alice",
  day = 0,
  text = `Message ${n}: 你好 🌿`,
) => ({
  kind: "message",
  key: `m${n}`,
  order: n,
  message: {
    id: `m${n}`,
    sequence: n,
    from,
    to: null,
    time: base + day * 86400 + n * 10,
    text,
    reactions: {},
    reply: null,
    mentions: [],
  },
});
const group = [message(1), message(2), message(3)];
const remove = (n) => (rows) => rows.filter((r) => r.message.sequence !== n);
const many = Array.from({ length: 1000 }, (_, i) =>
  message(i + 1, i % 4 ? "alice" : "bob"),
);
const cases = [
  { name: "Delete first grouped message", before: group, change: remove(1) },
  { name: "Delete middle grouped message", before: group, change: remove(2) },
  { name: "Delete last grouped message", before: group, change: remove(3) },
  {
    name: "Delete beside an open editor",
    before: group.slice(0, 2),
    change: remove(1),
    editor: 2,
  },
  {
    name: "Merge author groups",
    before: [message(1), message(2, "bob"), message(3)],
    change: remove(2),
  },
  {
    name: "Transfer a date divider",
    before: [message(1), message(2, "bob", 1), message(3, "bob", 1)],
    change: remove(2),
  },
  {
    name: "Delete an entire day",
    before: [message(1), message(2, "bob", 1), message(3, "carol", 2)],
    change: remove(2),
  },
  {
    name: "Short history with different authors",
    before: [message(1), message(2, "bob")],
    change: remove(1),
  },
  { name: "Delete the only message", before: [message(1)], change: remove(1) },
  {
    name: "Overflow becomes a short history",
    before: [
      message(1),
      message(2, "bob", 0, "Tall message\n".repeat(35)),
      message(3, "carol"),
    ],
    change: remove(2),
  },
  {
    name: "Delete a tall final message",
    before: [message(1), message(2, "bob", 0, "Tall message\n".repeat(35))],
    change: remove(2),
  },
  {
    name: "Delete near bottom of 1000 messages",
    before: many,
    change: remove(999),
  },
  {
    name: "Delete while reading older history",
    before: many,
    change: remove(502),
    read: 501,
  },
  {
    name: "Rapid grouped deletions",
    before: [...group, message(4), message(5)],
    change: remove(1),
    second: remove(2),
  },
  {
    name: "Arrival during a deletion",
    before: group,
    change: remove(1),
    second: (r) => [...r, message(4)],
  },
  {
    name: "Remote edit grows a grouped message",
    before: group,
    change: (r) =>
      r.map((e) =>
        e.key === "m2"
          ? {
              ...e,
              message: {
                ...e.message,
                text: "Edited text\n".repeat(10),
                edited: true,
              },
            }
          : e,
      ),
  },
  {
    name: "Remote edit shrinks a message",
    before: [
      message(1),
      message(2, "bob", 0, "Long text\n".repeat(10)),
      message(3),
    ],
    change: (r) =>
      r.map((e) =>
        e.key === "m2"
          ? { ...e, message: { ...e.message, text: "Edited", edited: true } }
          : e,
      ),
  },
  {
    name: "Older page loads without motion",
    before: group.slice(1),
    change: () => group,
    noMotion: true,
  },
  {
    name: "Switch conversation without motion",
    before: group,
    change: (r) =>
      r.map((e) => ({
        ...e,
        message: { ...e.message, text: "Other conversation" },
      })),
    switch: true,
    noMotion: true,
  },
  {
    name: "Reduced motion deletion",
    before: group,
    change: remove(1),
    reduced: true,
    noMotion: true,
  },
];
let reduced = false;
const matchMedia = window.matchMedia.bind(window);
window.matchMedia = (query) => {
  const result = matchMedia(query);
  if (!query.includes("prefers-reduced-motion")) return result;
  return new Proxy(result, {
    get(target, property) {
      if (property === "matches" && reduced) return true;
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
};
const frames = (count) =>
  new Promise((resolve) => {
    function frame() {
      if (--count <= 0) resolve();
      else requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  });
function snapshot(viewport) {
  const clip = viewport.querySelector(".timeline-rows");
  const rect = viewport.getBoundingClientRect();
  const clipRect = clip.getBoundingClientRect();
  const style = getComputedStyle(clip);
  const clipped = style.overflowY === "clip";
  const margin = parseFloat(style.overflowClipMargin) || 0;
  return {
    height: viewport.clientHeight,
    gap: viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop,
    active: viewport.getAnimations({ subtree: true }).length,
    ghosts: viewport.querySelectorAll(".message-exit").length,
    rows: [...viewport.querySelectorAll("[data-virtual-key]")].map((el) => {
      const body = el.querySelector(".message-body, .message-editor");
      const b = body?.getBoundingClientRect();
      return {
        id: el.dataset.virtualKey,
        top: b?.top,
        height: b?.height,
        visible: b
          ? b.top >= rect.top &&
            b.bottom <= rect.bottom &&
            (!clipped ||
              (b.top >= clipRect.top - margin &&
                b.bottom <= clipRect.bottom + margin))
          : false,
      };
    }),
  };
}
function assess(spec, samples, viewport, bottom) {
  const first = samples[0];
  const last = samples.at(-1);
  const failures = [];
  if (
    last.active ||
    last.ghosts ||
    viewport.querySelector(".timeline-rows").style.minHeight
  )
    failures.push("unfinished motion");
  if (!spec.read && (last.gap > 2 || !bottom.current))
    failures.push(`lost bottom: ${last.gap}`);
  const ids = [...viewport.querySelectorAll("[id]")].map((e) => e.id);
  if (ids.length !== new Set(ids).size) failures.push("duplicate IDs");
  if (spec.noMotion && samples.some((s) => s.active))
    failures.push("unexpected animation");
  if (!spec.noMotion && !samples.some((s) => s.active))
    failures.push("missing animation");
  if (!spec.noMotion) {
    const initialKeys = first.rows.map((r) => r.id).join(",");
    const changed = samples.find(
      (s) => s.active || s.rows.map((r) => r.id).join(",") !== initialKeys,
    );
    for (const original of first.rows) {
      const start = changed?.rows.find((r) => r.id === original.id);
      const final = last.rows.find((r) => r.id === original.id);
      if (
        start?.top === undefined ||
        final?.top === undefined ||
        original.top === undefined
      )
        continue;
      // A whole displacement in the first rendered frame is a snap, even if
      // later frames are monotonic and an unrelated fade is still running.
      if (
        Math.abs(start.top - original.top) >
        Math.max(1.5, Math.abs(final.top - original.top) * 0.25)
      )
        failures.push(`${original.id}: first-frame snap`);
    }
  }
  if (!spec.noMotion && !spec.second) {
    for (const original of first.rows) {
      const final = last.rows.find((r) => r.id === original.id);
      if (!final || original.top === undefined || final.top === undefined)
        continue;
      const points = samples
        .map((s) => s.rows.find((r) => r.id === original.id))
        .filter(Boolean);
      const low = Math.min(original.top, final.top) - 1;
      const high = Math.max(original.top, final.top) + 1;
      const direction = Math.sign(final.top - original.top);
      if (points.some((p) => p.top < low || p.top > high))
        failures.push(`${original.id}: position overshoot`);
      if (
        points.some(
          (p, i) =>
            i && direction && (p.top - points[i - 1].top) * direction < -1,
        )
      )
        failures.push(`${original.id}: reversed direction`);
      if (original.visible && final.visible && points.some((p) => !p.visible))
        failures.push(`${original.id}: clipped mid-transition`);
    }
  }
  return {
    name: spec.name,
    pass: !failures.length,
    failures,
    mounted: last.rows.length,
    samples,
  };
}
function Fixture() {
  const [entries, setEntries] = useState(group);
  const [revision, setRevision] = useState(1);
  const [scope, setScope] = useState("initial");
  const [ready, setReady] = useState(true);
  const [compact, setCompact] = useState(false);
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState([]);
  const [selected, setSelected] = useState(0);
  const [detail, setDetail] = useState("");
  const viewport = useRef(null),
    handle = useRef(null),
    end = useRef(null),
    bottom = useRef(true),
    serial = useRef(0);
  useLayoutEffect(() => {
    if (bottom.current && viewport.current)
      viewport.current.scrollTop =
        viewport.current.scrollHeight - viewport.current.clientHeight;
  }, [entries]);
  async function run(spec) {
    reduced = !!spec.reduced;
    bottom.current = true;
    setScope(`case-${++serial.current}`);
    setEntries(spec.before);
    setReady(true);
    setRevision((n) => n + 1);
    await frames(6);
    if (spec.read) {
      bottom.current = false;
      handle.current.scrollToSequence(spec.read);
      await frames(4);
    }
    if (spec.editor) {
      document
        .querySelector(
          `#message-m${spec.editor} button[aria-label="Edit your message"]`,
        )
        .click();
      await frames(12);
    }
    const v = viewport.current;
    const samples = [{ ms: 0, ...snapshot(v) }];
    const start = performance.now();
    setReady(false);
    const firstChange = setTimeout(() => {
      setEntries(spec.change(spec.before));
      setReady(true);
      if (!spec.noMotion) setRevision((n) => n + 1);
      if (spec.switch) setScope(`case-${++serial.current}`);
    }, 100);
    const secondChange = spec.second
      ? setTimeout(() => {
          setEntries((r) => spec.second(r));
          setRevision((n) => n + 1);
        }, 150)
      : undefined;
    while (performance.now() - start < 450) {
      await frames(1);
      samples.push({ ms: performance.now() - start, ...snapshot(v) });
    }
    clearTimeout(firstChange);
    clearTimeout(secondChange);
    const result = assess(spec, samples, v, bottom);
    setDetail(JSON.stringify(result));
    if (spec.editor) {
      document.querySelector('.message-editor button[type="button"]').click();
      await frames(12);
    }
    return result;
  }
  async function runCases(all) {
    setRunning(true);
    setResults([]);
    try {
      const list = all ? cases : [cases[selected]];
      const complete = [];
      for (const spec of list) {
        setDetail(`Running ${spec.name}`);
        complete.push(await run(spec));
        setResults([...complete]);
      }
    } catch (error) {
      setDetail(String(error));
      setResults((previous) => [
        ...previous,
        {
          name: "Harness failure",
          pass: false,
          failures: [String(error)],
          mounted: 0,
          samples: [],
        },
      ]);
    } finally {
      setRunning(false);
    }
  }
  return h(
    "div",
    {
      style: {
        height: "100vh",
        maxWidth: compact ? "390px" : "1000px",
        margin: "auto",
        display: "flex",
        flexDirection: "column",
      },
    },
    h(
      "div",
      { style: { padding: 8, display: "flex", gap: 8, flexWrap: "wrap" } },
      h(
        "button",
        { disabled: running, onClick: () => runCases(true) },
        "Run all cases",
      ),
      h(
        "button",
        { disabled: running, onClick: () => runCases(false) },
        "Run current case",
      ),
      h(
        "label",
        null,
        h("input", {
          type: "checkbox",
          checked: compact,
          disabled: running,
          onChange: (e) => setCompact(e.currentTarget.checked),
        }),
        "Compact",
      ),
      h(
        "select",
        {
          "aria-label": "Motion case",
          value: selected,
          disabled: running,
          onChange: (e) => setSelected(Number(e.currentTarget.value)),
        },
        cases.map((c, i) => h("option", { value: i, key: c.name }, c.name)),
      ),
    ),
    h(
      "output",
      {
        "data-testid": "summary",
        style: { height: 42, flexShrink: 0, fontSize: 12 },
      },
      running
        ? "Running"
        : results.length
          ? `${results.every((r) => r.pass) ? "PASS" : "FAIL"}: ${results.filter((r) => r.pass).length}/${results.length}`
          : "Ready",
    ),
    h(MessageList, {
      entries,
      username: "alice",
      pending: false,
      debug: false,
      permissions: [
        "r:message.read",
        "w:message.create",
        "w:message.edit.own",
        "w:message.react",
        "w:message.retract.own",
      ],
      commands: ["/edit", "/reply", "/react", "/retract"].map((name) => ({
        name,
        usage: name,
        description: name,
        section: "Messages",
        admin: false,
        console: false,
      })),
      unreadBoundary: null,
      motionScope: scope,
      revision,
      historyReady: ready,
      atBottom: bottom,
      historyElement: viewport,
      virtualTimeline: handle,
      end,
      onScroll: () => {
        const v = viewport.current;
        bottom.current = v.scrollHeight - v.clientHeight - v.scrollTop < 32;
      },
      onReact: () => {},
      onReply: () => {},
      onDelete: () => {},
      onEdit: async () => true,
    }),
    h(
      "details",
      { style: { maxHeight: 180, overflow: "auto", flexShrink: 0 } },
      h("summary", null, "Regression results"),
      h(
        "ul",
        { "data-testid": "results" },
        results.map((r) =>
          h(
            "li",
            { key: r.name },
            `${r.pass ? "PASS" : "FAIL"} ${r.name}: ${r.failures.join(", ") || `${r.mounted} mounted rows`}`,
          ),
        ),
      ),
      h("pre", { "data-testid": "detail" }, detail),
    ),
  );
}
render(h(Fixture), document.getElementById("app"));
