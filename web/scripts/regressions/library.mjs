import { h, render } from "preact";
import { useRef, useState } from "preact/hooks";
import { Settings } from "../../src/components/Settings.tsx";
import { StickerSettings } from "../../src/components/settings/StickerSettings.tsx";
import { LibraryPicker } from "../../src/components/LibraryPicker.tsx";
import { ManagedAttachment } from "../../src/components/ManagedAttachment.tsx";
import { LibrarySourceContext } from "../../src/components/LibrarySourceContext.ts";
import { Composer } from "../../src/components/Composer.tsx";
import { CollapsibleMessage } from "../../src/components/CollapsibleMessage.tsx";
import { ContentMotionContext } from "../../src/hooks/useContentResizeMotion.ts";
import { useVirtualTimeline } from "../../src/hooks/useVirtualTimeline.ts";
import { useMediaUploads } from "../../src/hooks/useMediaUploads.ts";
import { httpImageCache } from "../../src/features/conversation/httpImageCache.ts";
import { api, ApiError } from "../../src/api/client.ts";
import { serverBaseUrl } from "../../src/api/server.ts";

const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const sticker = {
  media_id: id,
  filename: "wave.gif",
  content_type: "image/gif",
  size: 800,
  available: true,
  owned: true,
  saved: true,
  owner_count: 2,
};
const page = (items) => ({
  items,
  next_cursor: null,
  usage: 800,
  quota: 10000,
  cleanup_bytes: 0,
  cleanup_objects: 0,
  oldest_cleanup: null,
  uploads_paused: false,
});

function PickerComposer() {
  const input = useRef(null);
  const [draft, edit] = useState("hello world");
  return h(Composer, {
    account: "picker-test",
    minimumComposerHeight: 40,
    actualComposerHeight: 40,
    hints: [],
    activeHint: 0,
    replyTarget: null,
    input,
    consoleView: false,
    draft,
    pending: false,
    canWrite: true,
    edit,
    startResize() {},
    moveResize() {},
    endResize() {},
    resizeComposer() {},
    acceptHint() {},
    cancelReply() {},
    send(event) {
      event.preventDefault();
    },
    keydown() {},
    selectCaret() {},
  });
}

export async function libraryRegressions({
  test,
  tests,
  until,
  check,
  sleep,
  empty,
}) {
  const click = (label) => {
    const button = tests.querySelector(`[aria-label="${label}"]`);
    check(button, `Missing ${label} button`);
    button.click();
  };
  async function manage() {
    click("Stickers");
    await until(() => tests.querySelector('[aria-label="Collections"]'));
  }
  await test("media library repeated icon clicks reverse picker motion immediately", async () => {
    api.mediaConfig = async () => ({ enabled: false, max_size: 0 });
    api.library = async () => page([]);
    render(h(PickerComposer), tests);
    click("Open media library");
    await until(() => tests.querySelector(".library-picker"));
    const picker = tests.querySelector(".library-picker");
    let animation = picker.getAnimations()[0];
    check(animation, "opening has an animation");
    animation.finish();
    await animation.finished;
    for (let turn = 0; turn < 6; turn++) {
      click("Open media library");
      await until(() => picker.getAnimations()[0] !== animation);
      animation = picker.getAnimations()[0];
      check(animation, "each click immediately starts the requested direction");
      check(
        animation.playbackRate > 0 &&
          animation.effect.getTiming().easing === "ease-out",
        "closing and reopening both start promptly with ease-out",
      );
      check(
        animation.effect.getTiming().duration <= 120,
        "interrupted motion never restarts a full long transition",
      );
      animation.pause();
      animation.currentTime = Number(animation.effect.getTiming().duration) / 2;
      const painted = getComputedStyle(picker).opacity;
      const bounds = picker.getBoundingClientRect();
      click("Open media library");
      await until(() => picker.getAnimations()[0] !== animation);
      const reopened = picker.getAnimations()[0];
      reopened.pause();
      reopened.currentTime = 0;
      check(
        tests.querySelector(".library-picker") === picker,
        "reopening retains the mounted picker",
      );
      check(
        tests
          .querySelector('[aria-label="Open media library"]')
          .getAttribute("aria-expanded") === "true",
        "the latest click reopens instead of waiting for dismissal",
      );
      check(
        Math.abs(Number(getComputedStyle(picker).opacity) - Number(painted)) <
          0.02,
        "direction changes continue from the painted opacity without a jump",
      );
      const continued = picker.getBoundingClientRect();
      check(
        Math.abs(continued.width - bounds.width) < 1 &&
          Math.abs(continued.top - bounds.top) < 1,
        "reversal also preserves painted scale and placement",
      );
      reopened.finish();
      await reopened.finished;
      animation = reopened;
    }
    click("Open media library");
    await until(() => !tests.querySelector(".library-picker"));
    const matchMedia = window.matchMedia;
    window.matchMedia = (query) => {
      const result = matchMedia.call(window, query);
      if (query === "(prefers-reduced-motion: reduce)")
        Object.defineProperty(result, "matches", { value: true });
      return result;
    };
    try {
      click("Open media library");
      await until(() => tests.querySelector(".library-picker"));
      check(
        tests.querySelector(".library-picker").getAnimations().length === 0,
        "reduced motion reveals immediately without an animation",
      );
      click("Open media library");
      await until(() => !tests.querySelector(".library-picker"));
    } finally {
      window.matchMedia = matchMedia;
    }
  });
  await test("media library picker and quota share GETs", async () => {
    const calls = [],
      pending = [];
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = (view, after, signal) => {
      calls.push({ view, after, signal });
      return new Promise((resolve) => pending.push(() => resolve(page([]))));
    };
    render(
      h(
        "div",
        null,
        h(LibraryPicker, {
          account: "shared-library-page",
          canSelect: true,
          select() {},
          close() {},
        }),
        h(StickerSettings, { account: "shared-library-page" }),
      ),
      tests,
    );
    click("Stickers");
    await until(() => pending.length === 2);
    check(
      calls.filter((call) => call.view === "library").length === 1,
      "the collection and quota share one initial library GET",
    );
    check(
      calls.filter((call) => call.view === "recent").length === 1,
      "Recent keeps its distinct query",
    );
    for (const resolve of pending.splice(0)) resolve();
    await until(() => tests.querySelector(".library-quota"));
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => pending.length === 2);
    check(
      calls.filter((call) => call.view === "library").length === 2,
      "invalidation starts one fresh library GET for both consumers",
    );
    for (const resolve of pending.splice(0)) resolve();
    await sleep(30);
  });
  await test("media library has minimal sticker sections with independent pagination and Account quota", async () => {
    const later = {
      ...sticker,
      media_id: "d5cd8a44-d8b7-4c63-91fe-3cc63f106179",
      filename: "hidden-cat.gif",
    };
    const calls = [];
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view, after) => {
      calls.push({ view, after });
      return after
        ? page([later])
        : {
            ...page([sticker]),
            next_cursor: view === "recent" ? null : "next",
          };
    };
    render(
      h(LibraryPicker, { canSelect: true, select() {}, close() {} }),
      tests,
    );
    click("Stickers");
    await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
    check(
      !tests.querySelector(".library-quota"),
      "recent stickers have no quota display",
    );
    await until(() => tests.querySelector(".library-preview"));
    check(
      !tests.querySelector(".library-quota"),
      "saved sticker browsing has no quota display",
    );
    check(
      [...tests.querySelectorAll(".library-content section h3")]
        .map((title) => title.textContent)
        .join(",") === "Recent,Collections",
      "sticker sections use small headings in order",
    );
    check(
      !tests.querySelector(
        ".library-topbar, .library-search, [role=searchbox]",
      ),
      "picker has no toolbar or search widgets",
    );
    const recentPreview = tests.querySelector(
      '[aria-label="Recent"] .library-sticker-tile',
    );
    const addPreview = tests.querySelector(".library-upload-tile");
    let collections = tests.querySelector('[aria-label="Collections"]');
    [...collections.querySelectorAll("button")]
      .find((button) => button.textContent === "Next page")
      .click();
    await until(() =>
      tests.querySelector(
        '[aria-label="Collections"] [aria-label="Use hidden-cat.gif"]',
      ),
    );
    collections = tests.querySelector('[aria-label="Collections"]');
    check(
      tests.querySelector('[aria-label="Recent"] .library-sticker-tile') ===
        recentPreview &&
        tests.querySelector(".library-upload-tile") === addPreview,
      "pagination preserves Recent and Add rather than rebuilding the preview",
    );
    check(
      calls.some((call) => call.view === "library" && call.after === "next"),
      "collections can reach later pages",
    );
    check(
      tests.querySelector('[aria-label="Recent"] [aria-label="Use wave.gif"]'),
      "paginating collections preserves recent stickers",
    );
    check(
      !tests.querySelector(
        '.library-management, .library-quota, [aria-label="Manage stickers"]',
      ),
      "picker contains no management or quota controls",
    );
    check(
      tests
        .querySelector('[aria-label="Collections"] .library-grid')
        .lastElementChild.classList.contains("library-upload-tile"),
      "the add-sticker square stays after the final sticker",
    );
    check(
      !calls.some((call) => call.view === "owned"),
      "the picker does not fetch a duplicate owned section",
    );
    const acquiredGrid = collections.querySelector(".library-grid");
    const uploadTile = acquiredGrid.querySelector(".library-upload-tile");
    const existingTile = acquiredGrid.querySelector(".library-sticker-tile");
    const uploadBounds = uploadTile.getBoundingClientRect();
    const existingBounds = existingTile.getBoundingClientRect();
    check(
      Math.abs(uploadBounds.top - existingBounds.top) < 1 &&
        uploadBounds.left > existingBounds.left &&
        Math.abs(uploadBounds.width - uploadBounds.height) < 1,
      "upload placeholder follows collection media as a compact square",
    );

    render(
      h(Settings, {
        state: empty,
        pending: false,
        onAccountCommand: async () => ({ kind: "notice", id: 1, text: "ok" }),
        onClose() {},
      }),
      tests,
    );
    await until(() =>
      tests.querySelector(
        '.settings-content[aria-label="Account"] .library-quota',
      ),
    );
    check(
      tests
        .querySelector(".library-quota")
        .title.includes("800 / 10,000 bytes"),
      "management exposes account media quota",
    );
  });
  await test("media library edit mode gates pointer and keyboard controls", async () => {
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async () => page([sticker]);
    render(
      h(LibraryPicker, {
        account: "edit-mode",
        canSelect: true,
        select() {},
        close() {},
      }),
      tests,
    );
    await manage();
    await until(() => tests.querySelector(".library-sticker-actions"));
    const edit = tests.querySelector(".library-edit");
    const actions = tests.querySelector(".library-sticker-actions");
    const remove = actions.querySelector("button");
    check(
      edit.textContent === "Edit" &&
        edit.getAttribute("aria-pressed") === "false",
      "Edit begins off beside Collections",
    );
    remove.focus();
    check(
      document.activeElement !== remove && actions.inert,
      "browsing skips manipulation controls in keyboard order",
    );
    edit.click();
    await until(() => !actions.inert);
    remove.focus();
    await until(
      () =>
        document.activeElement === remove &&
        getComputedStyle(actions).opacity === "1",
      () =>
        JSON.stringify({
          focus: document.activeElement?.outerHTML,
          opacity: getComputedStyle(actions).opacity,
          inert: actions.inert,
        }),
    );
    edit.click();
    await until(() => actions.inert);
    await until(() => getComputedStyle(actions).opacity === "0");
    check(
      getComputedStyle(actions).opacity === "0" && edit.textContent === "Edit",
      "Done restores clean sticker browsing",
    );
  });
  await test("media library motion coordinates removal wrapping reordering and interrupted resize", async () => {
    const initial = Array.from({ length: 4 }, (_, index) => ({
      ...sticker,
      media_id: `b5cd8a44-d8b7-4c63-91fe-3cc63f10617${index}`,
      filename: `motion-${index}.png`,
      available: false,
      owned: false,
    }));
    let items = initial;
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view) => page(view === "recent" ? [] : items);
    render(
      h(LibraryPicker, {
        account: "layout-motion",
        canSelect: true,
        select() {},
        close() {},
      }),
      tests,
    );
    await manage();
    await until(
      () =>
        tests.querySelectorAll(".library-grid > .library-sticker-tile")
          .length === 4,
    );
    const picker = tests.querySelector(".library-picker");
    picker.style.width = "300px";
    await sleep(300);
    const root = tests.querySelector(".library-content");
    const grid = tests.querySelector(".library-grid");
    const add = grid.querySelector(".library-upload-tile");
    const start = add.getBoundingClientRect();
    items = initial.slice(0, 3);
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(
      () =>
        grid.querySelectorAll(".library-sticker-tile").length === 3 &&
        add.getAnimations().length,
    );
    let motion = add.getAnimations()[0];
    check(
      motion.effect.getTiming().duration === 180,
      "deletion retains its 180ms row movement",
    );
    motion.pause();
    motion.currentTime = 0;
    let rect = add.getBoundingClientRect();
    check(
      Math.abs(rect.top - start.top) < 1 &&
        Math.abs(rect.left - start.left) < 1,
      "row removal starts Add at its previous painted position",
    );
    check(
      root.querySelector(".library-motion-exit") &&
        !grid.querySelector(".library-motion-exit"),
      "removed preview fades outside flex layout",
    );
    motion.currentTime = 75;
    const interrupted = add.getBoundingClientRect();
    items = [initial[2], initial[0], initial[1], initial[3]];
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(
      () =>
        grid.querySelectorAll(".library-sticker-tile").length === 4 &&
        add.getAnimations()[0] !== motion,
    );
    motion = add.getAnimations()[0];
    check(
      motion.effect.getTiming().duration === 120,
      "additions and reordering use the faster 120ms timing",
    );
    motion.pause();
    motion.currentTime = 0;
    rect = add.getBoundingClientRect();
    check(
      Math.abs(rect.top - interrupted.top) < 1 &&
        Math.abs(rect.left - interrupted.left) < 1,
      "new additions retarget an interrupted row movement without a jump",
    );
    const first = grid.firstElementChild;
    check(
      first.querySelector("button").getAttribute("aria-label") ===
        "Use motion-2.png",
      "reordering preserves API order",
    );
    const previousResizeMotion = first.getAnimations()[0];
    if (previousResizeMotion) {
      previousResizeMotion.pause();
      previousResizeMotion.currentTime = 60;
    }
    const beforeResize = first.getBoundingClientRect();
    picker.style.width = "420px";
    await until(
      () =>
        first.getAnimations().length &&
        first.getAnimations()[0] !== previousResizeMotion,
    );
    const resizeMotion = first.getAnimations()[0];
    check(
      resizeMotion.effect.getTiming().duration === 120,
      "row resizing uses 120ms timing",
    );
    resizeMotion.pause();
    resizeMotion.currentTime = 0;
    const resized = first.getBoundingClientRect();
    check(
      Math.abs(resized.width - beforeResize.width) < 1 &&
        Math.abs(resized.top - beforeResize.top) < 1,
      "row resize preserves the current painted size before interpolation",
    );
    resizeMotion.currentTime = 60;
    const paintBeforeNotification = first.getBoundingClientRect();
    first.append(document.createComment("unrelated content notification"));
    await sleep(30);
    check(
      first.getAnimations()[0] === resizeMotion &&
        resizeMotion.currentTime === 60,
      "unchanged observer notifications retain the animation and its progress",
    );
    const paintAfterNotification = first.getBoundingClientRect();
    check(
      Math.abs(paintAfterNotification.left - paintBeforeNotification.left) <
        0.5 &&
        Math.abs(paintAfterNotification.width - paintBeforeNotification.width) <
          0.5,
      "unchanged notifications preserve the painted position and size",
    );
    for (const animation of root.getAnimations({ subtree: true }))
      animation.finish();
    items = [];
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => grid.children.length === 1);
    check(
      grid.firstElementChild === add &&
        root === tests.querySelector(".library-content"),
      "removing the final image preserves Add and the preview container",
    );
    await sleep(250);
    check(
      !root.querySelector(".library-motion-exit") &&
        !add.getAnimations().length,
      `exit layers and animations clean up after the final row settles: ${JSON.stringify({ ghosts: root.querySelectorAll(".library-motion-exit").length, animations: add.getAnimations().map((a) => ({ state: a.playState, time: a.currentTime, keys: a.effect.getKeyframes() })) })}`,
    );
  });
  await test("media library reduced motion cancels row transitions and releases observers", async () => {
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
    let items = [sticker];
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view) => page(view === "recent" ? [] : items);
    try {
      render(
        h(LibraryPicker, {
          account: "motion-preference",
          canSelect: true,
          select() {},
          close() {},
        }),
        tests,
      );
      await manage();
      await until(() => tests.querySelector(".library-sticker-tile"));
      await sleep(250);
      const root = tests.querySelector(".library-content");
      items = [];
      window.dispatchEvent(new Event("commonroom-library-change"));
      await until(() => root.querySelector(".library-motion-exit"));
      reduced = true;
      for (const listener of listeners) listener();
      check(
        !root.querySelector(".library-motion-exit") &&
          root.getAnimations({ subtree: true }).length === 0,
        "changing reduced motion cancels movement and removes exit copies immediately",
      );
      items = [sticker];
      window.dispatchEvent(new Event("commonroom-library-change"));
      await until(() =>
        root.querySelector(".library-grid > .library-sticker-tile"),
      );
      check(
        root.getAnimations({ subtree: true }).length === 0,
        "reduced motion additions do not animate",
      );
      render(null, tests);
      check(
        listeners.size === 0,
        "unmount releases motion preference listeners",
      );
    } finally {
      window.matchMedia = original;
    }
  });
  await test("media library panel changes fade and outgoing controls become inert", async () => {
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async () => page([{ ...sticker, owner_count: 1 }]);
    render(
      h(LibraryPicker, {
        account: "panel-motion",
        canSelect: true,
        select() {},
        close() {},
      }),
      tests,
    );
    await manage();
    await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
    await sleep(250);
    const outgoing = tests.querySelector(".library-content");
    click("Emojis");
    await until(() => outgoing.inert && outgoing.getAnimations().length);
    check(outgoing.isConnected, "outgoing mode stays mounted for its fade");
    await until(() => !outgoing.isConnected);
    click("Stickers");
    await until(() =>
      tests.querySelector(
        '.library-content:not([inert]) [aria-label="Remove wave.gif"]',
      ),
    );
    tests.querySelector(".library-edit").click();
    await sleep(20);
    click("Remove wave.gif");
    await until(() => tests.querySelector(".library-confirmation-layer"));
    const layer = tests.querySelector(".library-confirmation-layer");
    await until(() => layer.getAnimations().length);
    await sleep(160);
    tests.querySelector(".library-confirmation-keep").click();
    await until(() => layer.inert && layer.getAnimations().length);
    check(
      layer.isConnected,
      "confirmation exits with an inert fade rather than disappearing",
    );
    await until(() => !layer.isConnected);
  });
  for (const disappeared of [false, true])
    await test(`media library confirmation restores focus when trigger ${disappeared ? "disappears" : "remains"}`, async () => {
      let items = [{ ...sticker, owner_count: 1 }];
      api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
      api.library = async () => page(items);
      render(
        h(LibraryPicker, {
          account: `return-focus-${disappeared}`,
          canSelect: true,
          select() {},
          close() {},
        }),
        tests,
      );
      await manage();
      await until(() => tests.querySelector(".library-edit"));
      tests.querySelector(".library-edit").click();
      await until(() => !tests.querySelector(".library-sticker-actions").inert);
      const trigger = tests.querySelector('[aria-label="Remove wave.gif"]');
      trigger.focus();
      trigger.click();
      await until(() => tests.querySelector(".library-confirmation-keep"));
      if (disappeared) {
        items = [];
        window.dispatchEvent(new Event("commonroom-library-change"));
        await until(() => !trigger.isConnected);
      }
      const keep = tests.querySelector(".library-confirmation-keep");
      keep.focus();
      keep.click();
      await until(() => !tests.querySelector(".library-confirmation"));
      check(
        document.activeElement ===
          (disappeared ? tests.querySelector(".library-edit") : trigger),
        "Keep sticker returns keyboard focus to the trigger or Edit fallback",
      );
    });
  await test("media library packs borderless previews by natural image proportions", async () => {
    const service = Object.getOwnPropertyDescriptor(navigator, "serviceWorker");
    const load = httpImageCache.load;
    const shapes = [
      [180, 100],
      [180, 100],
      [60, 100],
      [75, 100],
      [100, 100],
      [180, 100],
      [60, 100],
    ];
    const items = shapes.map((_, index) => ({
      ...sticker,
      media_id: `a5cd8a44-d8b7-4c63-91fe-3cc63f10617${index}`,
      filename: `layout-${index}.png`,
    }));
    Object.defineProperty(navigator, "serviceWorker", {
      configurable: true,
      value: undefined,
    });
    httpImageCache.load = async (href) => {
      const index = items.findIndex((item) => href.includes(item.media_id));
      const [width, height] = shapes[index];
      return URL.createObjectURL(
        new Blob(
          [
            `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect width="100%" height="100%" fill="teal" /></svg>`,
          ],
          { type: "image/svg+xml" },
        ),
      );
    };
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view) => page(view === "recent" ? [] : items);
    try {
      render(
        h(LibraryPicker, {
          account: "natural-layout",
          canSelect: true,
          select() {},
          close() {},
        }),
        tests,
      );
      await manage();
      const picker = tests.querySelector(".library-picker");
      await until(
        () =>
          picker.querySelectorAll(".library-preview img").length ===
            shapes.length &&
          [...picker.querySelectorAll(".library-preview img")].every(
            (image) => image.complete && image.naturalWidth,
          ),
      );
      for (const width of [420, 300]) {
        picker.style.width = `${width}px`;
        await sleep(350);
        const previews = [...picker.querySelectorAll(".library-preview")];
        const grid = picker
          .querySelector(".library-grid")
          .getBoundingClientRect();
        previews.forEach((preview, index) => {
          const bounds = preview.getBoundingClientRect();
          const style = getComputedStyle(preview);
          check(
            Math.abs(
              bounds.width / bounds.height -
                shapes[index][0] / shapes[index][1],
            ) < 0.02,
            "preview keeps its natural aspect ratio without square padding",
          );
          check(
            style.borderTopWidth === "0px" && style.padding === "0px",
            "previews have no surrounding border or padding",
          );
          check(
            bounds.left >= grid.left - 1 && bounds.right <= grid.right + 1,
            "media stays inside the responsive row",
          );
        });
        check(
          previews[0].getBoundingClientRect().width >
            previews[2].getBoundingClientRect().width,
          "landscape previews receive more horizontal space than portraits",
        );
        check(
          previews
            .map((preview) => preview.getAttribute("aria-label"))
            .join(",") ===
            items.map((item) => `Use ${item.filename}`).join(","),
          "packing preserves collection and keyboard order",
        );
      }
    } finally {
      render(null, tests);
      httpImageCache.load = load;
      if (service) Object.defineProperty(navigator, "serviceWorker", service);
      else delete navigator.serviceWorker;
    }
  });
  await test("media library minimal emoji sections recents and animated composer insertion", async () => {
    const key = `commonroom-emoji:${serverBaseUrl()}:picker-test`;
    const saved = localStorage.getItem(key);
    localStorage.removeItem(key);
    api.mediaConfig = async () => ({ enabled: false, max_size: 0 });
    api.library = async () => page([sticker]);
    try {
      render(h(PickerComposer), tests);
      const input = tests.querySelector("textarea");
      input.focus();
      input.setSelectionRange(6, 11);
      const composerTop = tests
        .querySelector(".composer")
        .getBoundingClientRect().top;
      click("Open media library");
      for (let frame = 0; frame < 4; frame++) {
        await new Promise(requestAnimationFrame);
        check(
          Math.abs(
            tests.querySelector(".composer").getBoundingClientRect().top -
              composerTop,
          ) < 1,
          "first picker loading and reveal never shift the composer",
        );
      }
      await until(() =>
        tests.querySelector('[aria-label="Smileys & people"].emoji-grid'),
      );
      check(
        !tests.querySelector(".library-topbar, .library-search"),
        "emoji picker has no toolbar or search",
      );
      check(
        !tests.textContent.includes("Media storage:"),
        "emoji browsing has no quota",
      );
      check(
        tests.querySelectorAll(".library-modes button").length === 2,
        "the picker only offers emoji and stickers",
      );
      check(
        tests.querySelector('[aria-label="Smileys & people"].emoji-grid'),
        "common smileys appear below Recent on first open",
      );
      check(
        [...tests.querySelectorAll(".library-modes button")].every(
          (button) =>
            !button.textContent.trim() && button.getAttribute("aria-label"),
        ),
        "mode buttons are icon-only with accessible labels",
      );
      check(
        tests.querySelector(".library-modes").getBoundingClientRect().width <
          100,
        "mode switch is compact",
      );
      const reveal = tests.querySelector("dialog").getAnimations()[0];
      check(
        reveal?.effect
          .getKeyframes()
          .some((frame) => frame.transform === "scale(0.15)"),
        "picker pops from its anchor like the user card",
      );
      await until(() => tests.querySelector('[aria-label="grinning face"]'));
      click("grinning face");
      await until(
        () => input.value === "hello 😀" && input.selectionStart === 8,
      );
      check(
        tests.querySelector("dialog"),
        "inserting emoji keeps the picker open for another choice",
      );
      click("grinning face with big eyes");
      await until(() => input.value === "hello 😀😃");
      await until(
        () =>
          tests.querySelectorAll('[aria-label="Recent emoji"] button')
            .length === 2,
      );
      check(
        tests.querySelector('[aria-label="Recent emoji"] button')
          .textContent === "😃",
        "recents are ordered by the last selection",
      );
      click("Open media library");
      check(
        tests.querySelector("dialog"),
        "the popup remains mounted during its close animation",
      );
      await until(() => !tests.querySelector("dialog"));
      click("Open media library");
      await until(
        () =>
          tests.querySelectorAll('[aria-label="Recent emoji"] button')
            .length === 2,
      );
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      await until(() => !tests.querySelector("dialog"));
      click("Open media library");
      await until(() => tests.querySelector("dialog"));
      document.body.dispatchEvent(
        new PointerEvent("pointerdown", { bubbles: true }),
      );
      await until(() => !tests.querySelector("dialog"));
    } finally {
      if (saved === null) localStorage.removeItem(key);
      else localStorage.setItem(key, saved);
    }
  });
  await test("media library caches sticker pages by account and keeps loading inside the content", async () => {
    let waiting = [];
    const calls = [];
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = (view) => {
      calls.push(view);
      return new Promise((resolve) =>
        waiting.push(() => resolve(page([sticker]))),
      );
    };
    const mount = (account) =>
      render(
        h(LibraryPicker, { account, canSelect: true, select() {}, close() {} }),
        tests,
      );
    const flush = () => {
      for (const resolve of waiting) resolve();
      waiting = [];
    };
    mount("cache-test-a");
    click("Stickers");
    await until(() => waiting.length === 2);
    check(
      tests
        .querySelector(".library-content:not([inert])")
        .textContent.includes("Loading stickers") &&
        tests.querySelectorAll(".library-modes button").length === 2,
      "cache-miss loading stays inside content with the mode switch mounted",
    );
    check(
      !tests.querySelector(".library-grid"),
      "cold loading hides every media grid",
    );
    waiting.shift()();
    await sleep(30);
    check(
      !tests.querySelector(".library-grid") &&
        tests.textContent.includes("Loading stickers"),
      "partial network responses cannot animate a half-loaded grid",
    );
    const bounds = {
      width: tests.querySelector("dialog").offsetWidth,
      height: tests.querySelector("dialog").offsetHeight,
    };
    flush();
    await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
    click("Emojis");
    await until(() =>
      tests.querySelector(".library-content:not([inert]) .emoji-grid"),
    );
    click("Stickers");
    await until(
      () =>
        tests.querySelector(
          '.library-content:not([inert]) [aria-label="Use wave.gif"]',
        ) &&
        !tests
          .querySelector(".library-content:not([inert])")
          .textContent.includes("Loading"),
    );
    await sleep(80);
    check(
      waiting.length === 0 && calls.length === 2,
      "fresh tab switches issue no GETs",
    );
    check(
      tests.querySelector('[aria-label="Use wave.gif"]') &&
        !tests
          .querySelector(".library-content:not([inert])")
          .textContent.includes("Loading"),
      "switching tabs displays fresh cached stickers",
    );
    flush();
    await sleep(20);
    render(null, tests);
    mount("cache-test-a");
    click("Stickers");
    await until(
      () =>
        tests.querySelector(
          '.library-content:not([inert]) [aria-label="Use wave.gif"]',
        ) &&
        !tests
          .querySelector(".library-content:not([inert])")
          .textContent.includes("Loading"),
    );
    await sleep(80);
    check(
      waiting.length === 0 && calls.length === 2,
      "fresh picker reopening issues no GETs",
    );
    check(
      tests.querySelector('[aria-label="Use wave.gif"]'),
      "reopening reuses the account's cached pages",
    );
    flush();
    await sleep(20);
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => waiting.length === 2);
    check(
      tests.querySelector('[aria-label="Use wave.gif"]'),
      "media invalidation retains displayed tiles during revalidation",
    );
    flush();
    await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
    const nextBounds = {
      width: tests.querySelector("dialog").offsetWidth,
      height: tests.querySelector("dialog").offsetHeight,
    };
    check(
      Math.abs(bounds.height - nextBounds.height) < 1 &&
        Math.abs(bounds.width - nextBounds.width) < 1,
      "loading does not resize the floating picker",
    );
    check(
      getComputedStyle(tests.querySelector(".library-content:not([inert])"))
        .scrollbarWidth === "none",
      "scrollbar is hidden while content remains scrollable",
    );
    render(null, tests);
    mount("cache-test-b");
    click("Stickers");
    await until(() => waiting.length === 2);
    check(
      !tests.querySelector('[aria-label="Use wave.gif"]'),
      "another account cannot reuse the first account's cache",
    );
    flush();
    await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
    check(
      calls.length === 6,
      "only cold mounts and invalidation fetch both sections",
    );
  });
  await test("media library remembers the tab and keeps retired stickers removable", async () => {
    const account = "mode-test";
    const key = `commonroom-picker-mode:${serverBaseUrl()}:${account}`;
    const saved = localStorage.getItem(key);
    localStorage.removeItem(key);
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async () =>
      page([
        sticker,
        {
          ...sticker,
          media_id: "d5cd8a44-d8b7-4c63-91fe-3cc63f106179",
          filename: "retired.gif",
          available: false,
        },
      ]);
    const mount = () =>
      render(
        h(LibraryPicker, { account, canSelect: true, select() {}, close() {} }),
        tests,
      );
    try {
      mount();
      click("Stickers");
      await until(() => tests.querySelector('[aria-label="Use wave.gif"]'));
      check(
        tests.querySelector('[aria-label="Use retired.gif"]').disabled &&
          !tests.querySelector('[aria-label="Acquire retired.gif"]') &&
          !tests.querySelector('[aria-label="Remove retired.gif"]').disabled &&
          tests.textContent.includes("Media unavailable"),
        "retired stickers cannot be used or acquired but remain removable",
      );
      await until(() => localStorage.getItem(key) === "stickers");
      render(null, tests);
      mount();
      await until(() =>
        tests.querySelector('[aria-label="Stickers"][aria-pressed="true"]'),
      );
      const modes = tests.querySelector(".library-modes");
      const content = tests
        .querySelector(".library-content")
        .getBoundingClientRect();
      check(
        getComputedStyle(modes).position === "absolute" &&
          modes.getBoundingClientRect().top < content.bottom,
        "tab switch floats over the scrollable content",
      );
      click("Emojis");
      await until(() => localStorage.getItem(key) === "emoji");
      render(null, tests);
      mount();
      await until(() =>
        tests.querySelector('[aria-label="Emojis"][aria-pressed="true"]'),
      );
    } finally {
      if (saved === null) localStorage.removeItem(key);
      else localStorage.setItem(key, saved);
    }
  });
  await test("media library quota preserves the displayed value during refresh and reopens from account cache", async () => {
    let finish;
    api.library = async () => ({ ...page([]), usage: 800 });
    render(h(StickerSettings, { account: "quota-a" }), tests);
    await until(() => tests.querySelector(".library-quota"));
    const quota = tests.querySelector(".library-quota");
    const fill = tests.querySelector(".library-quota-fill");
    const track = tests.querySelector(".library-quota-track");
    check(
      track.dataset.used === "true" &&
        getComputedStyle(track, "::after").width === "3px" &&
        track.getAttribute("aria-valuenow") === "800",
      "nonzero usage has a visible marker with an exact accessible value",
    );
    api.library = () =>
      new Promise((resolve) => {
        finish = resolve;
      });
    click("Refresh media storage");
    await until(() => finish);
    check(
      tests.querySelector(".library-quota") === quota &&
        quota.title.includes("800 /"),
      "refresh preserves the quota widget and last value",
    );
    finish({ ...page([]), usage: 1500 });
    await until(() => quota.title.includes("1,500 /"));
    check(
      tests.querySelector(".library-quota-fill") === fill &&
        fill.style.transform === "scaleX(0.15)" &&
        getComputedStyle(fill).transitionProperty === "transform",
      "the same bar animates to the new usage",
    );
    render(null, tests);
    finish = undefined;
    render(h(StickerSettings, { account: "quota-a" }), tests);
    check(
      tests.querySelector(".library-quota").title.includes("1,500 /"),
      "reopening instantly displays the last cached usage",
    );
    await sleep(100);
    check(finish === undefined, "fresh quota reopening skips the network");
    render(null, tests);
    render(h(StickerSettings, { account: "quota-b" }), tests);
    check(
      !tests.querySelector(".library-quota"),
      "a different account never sees the cached quota",
    );
  });
  for (const scenario of ["first page", "later page", "remove before paging"])
    await test(`media library upload completes on ${scenario} without sending a message`, async () => {
      const original = window.XMLHttpRequest;
      let completed = false;
      let finishUpload;
      let allocations = 0;
      let abandoned = 0;
      api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
      const paginated = scenario !== "first page";
      const firstPage = Array.from({ length: 50 }, (_, index) => ({
        ...sticker,
        media_id: `00000000-0000-4000-8000-${index.toString(16).padStart(12, "0")}`,
        filename: `old-${index}.png`,
      }));
      api.library = async (view, after) =>
        view === "recent"
          ? page([])
          : paginated && !after
            ? { ...page(firstPage), next_cursor: "50" }
            : page(completed ? [sticker] : []);
      api.allocateMedia = async (conversation, filename, size) => {
        check(
          conversation === "@library" && filename === "wave.gif" && size === 6,
          "upload reserves a personal library asset",
        );
        allocations++;
        return {
          media_id: id,
          upload: {
            method: "PUT",
            url: "https://storage.test/upload",
            headers: { "Content-Type": "application/octet-stream" },
            expires_in: 300,
          },
        };
      };
      api.completeMedia = async () => {
        completed = true;
        return { ...sticker, path: `/api/media/${id}/file` };
      };
      api.abandonMedia = async () => {
        abandoned++;
        return { ok: true };
      };
      window.XMLHttpRequest = class {
        upload = {};
        open() {}
        setRequestHeader() {}
        send() {
          check(
            this.withCredentials === false,
            "storage receives no chat credentials",
          );
          finishUpload = () => {
            this.status = 200;
            this.onload();
          };
        }
      };
      let closed = false;
      let selected;
      try {
        render(
          h(LibraryPicker, {
            account: `upload-${scenario}`,
            canSelect: true,
            select(media) {
              selected = media;
            },
            close() {
              closed = true;
            },
          }),
          tests,
        );
        await manage();
        check(
          tests.querySelector('[aria-label="Collections"] .library-grid')
            .children.length === (paginated ? 51 : 1) &&
            tests
              .querySelector('[aria-label="Collections"] .library-grid')
              .lastElementChild.classList.contains("library-upload-tile"),
          "the current page ends with its upload tile",
        );
        await until(() => !tests.querySelector('[aria-label="Recent"]'));
        check(
          !tests.querySelector('[aria-label="Acquired by you"]'),
          "empty Recent is hidden and the owned section is absent",
        );
        const tile = tests
          .querySelector(".library-add-sticker")
          .getBoundingClientRect();
        check(
          Math.abs(tile.width - tile.height) < 1,
          "the upload tile is square",
        );
        await until(
          () =>
            tests.querySelector('input[type="file"]') &&
            !tests.querySelector('input[type="file"]').disabled,
        );
        const transfer = new DataTransfer();
        transfer.items.add(
          new File(["GIF89a"], "wave.gif", { type: "image/gif" }),
        );
        const input = tests.querySelector('input[type="file"]');
        input.files = transfer.files;
        input.dispatchEvent(new Event("change", { bubbles: true }));
        await until(
          () =>
            finishUpload && tests.querySelector(".library-pending-upload img"),
        );
        check(
          tests.querySelector(".library-upload-progress circle") &&
            !tests.textContent.includes("Uploading") &&
            !tests.textContent.includes("Preparing upload"),
          "pending upload shows a preview and circular indicator without text hints",
        );
        const pendingTile = tests.querySelector(".library-pending-upload");
        const addTile = tests.querySelector(".library-upload-tile");
        finishUpload();
        await until(
          () =>
            tests.querySelector(`[aria-label="Use ${sticker.filename}"]`) &&
            !tests.querySelector(".library-pending-upload"),
        );
        check(
          tests
            .querySelector(
              '[aria-label="Collections"] [aria-label="Use wave.gif"]',
            )
            .closest("li") === pendingTile &&
            tests.querySelector(".library-upload-tile") === addTile,
          "completion preserves the pending preview tile and Add DOM identity",
        );
        check(
          allocations === 1 && completed && !closed && !selected,
          "verified upload appears in the open picker",
        );
        check(
          tests.querySelector(
            '[aria-label="Collections"] [aria-label="Use wave.gif"]',
          ) && !tests.querySelector('[aria-label="Acquired by you"]'),
          "completion appears in Collections without an owned duplicate",
        );
        check(
          !tests.querySelector(".library-upload-progress") &&
            !tests.querySelector('[aria-label="Use wave.gif"]').disabled,
          "completed uploads are usable immediately without a spinner or cancel-upload control",
        );
        if (scenario === "remove before paging") {
          const actions = [];
          api.libraryChange = async (_id, action) => {
            actions.push(action);
            return { ok: true };
          };
          tests.querySelector(".library-edit").click();
          await until(
            () =>
              !tests
                .querySelector('[aria-label="Remove wave.gif"]')
                .closest(".library-sticker-actions").inert,
          );
          click("Remove wave.gif");
          await until(() =>
            tests.querySelector(".library-confirmation-remove"),
          );
          tests.querySelector(".library-confirmation-remove").click();
          await until(
            () =>
              !tests.querySelector('.library-grid [aria-label="Use wave.gif"]'),
          );
          check(
            actions.join(",") === "release,remove",
            "completed off-page uploads can be removed normally",
          );
          check(
            document.activeElement === tests.querySelector(".library-edit"),
            "successful deletion returns focus to Edit",
          );
        } else if (paginated) {
          [...tests.querySelectorAll('[aria-label="Collections"] button')]
            .find((button) => button.textContent === "Next page")
            .click();
          await until(
            () =>
              tests.querySelectorAll(
                '.library-grid [aria-label="Use wave.gif"]',
              ).length === 1 && !tests.textContent.includes("Next page"),
          );
          check(
            tests.querySelector(".library-upload-tile") === addTile,
            "paging to the canonical entry keeps Add and deduplicates the completed upload",
          );
          click("Use wave.gif");
          await until(() => selected);
          check(
            selected.media_id === id,
            "the completed upload selects its verified media ID",
          );
        }
        render(null, tests);
        await sleep(10);
        check(
          abandoned === 0,
          "closing the picker preserves completed ownership",
        );
      } finally {
        window.XMLHttpRequest = original;
      }
    });
  await test("media library capsule acquires ownership with themed circular controls and preserves errors", async () => {
    let item = { ...sticker, owned: false, owner_count: 1 };
    let failure = true;
    const actions = [];
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view) =>
      page(view === "owned" && !item.owned ? [] : [item]);
    api.libraryChange = async (_id, action) => {
      actions.push(action);
      if (failure) throw new ApiError(400, "Account media quota reached.");
      if (action === "acquire") item = { ...item, owned: true, owner_count: 2 };
      return { ok: true };
    };
    render(
      h(LibraryPicker, {
        account: "acquire-test",
        canSelect: true,
        select() {},
        close() {},
      }),
      tests,
    );
    await manage();
    await until(() => tests.querySelector('[aria-label="Acquire wave.gif"]'));
    tests.querySelector(".library-edit").click();
    const capsule = tests.querySelector(".library-sticker-actions");
    await until(() => !capsule.inert);
    const controls = [...capsule.querySelectorAll("button")];
    check(
      controls.length === 2 &&
        controls[0].classList.contains("library-remove-sticker") &&
        controls[1].classList.contains("library-acquire-sticker"),
      "ellipse places remove on the left and acquire on the right",
    );
    for (const theme of ["light", "dark"]) {
      const previous = document.documentElement.getAttribute("data-theme");
      document.documentElement.setAttribute("data-theme", theme);
      controls[0].focus();
      await until(() => getComputedStyle(capsule).opacity === "1");
      const style = getComputedStyle(capsule);
      const bounds = capsule.getBoundingClientRect();
      const circle = controls[0].getBoundingClientRect();
      check(
        bounds.width > bounds.height &&
          parseFloat(style.borderRadius) >= bounds.height / 2 &&
          Math.abs(circle.width - circle.height) < 1 &&
          style.opacity === "1",
        `${theme} capsule is an ellipse with circular keyboard-accessible buttons: ${JSON.stringify({ width: bounds.width, height: bounds.height, circleWidth: circle.width, circleHeight: circle.height, opacity: style.opacity, focused: document.activeElement === controls[0], focusVisible: controls[0].matches(":focus-visible"), inert: capsule.inert })}`,
      );
      check(
        style.backgroundColor ===
          getComputedStyle(tests.querySelector(".library-picker"))
            .backgroundColor,
        `${theme} capsule uses the themed picker surface`,
      );
      if (previous === null)
        document.documentElement.removeAttribute("data-theme");
      else document.documentElement.setAttribute("data-theme", previous);
    }
    check(
      controls[1].title.includes("800") && !controls[1].disabled,
      "acquisition advertises its full storage charge",
    );
    click("Acquire wave.gif");
    await until(
      () =>
        tests.textContent.includes("Account media quota reached.") &&
        tests.querySelector('[aria-label="Acquire wave.gif"]') &&
        !tests.querySelector('[aria-label="Acquire wave.gif"]').disabled,
    );
    check(
      !item.owned &&
        !tests.querySelector('[aria-label="Acquire wave.gif"]').disabled,
      "quota failure preserves ownership and permits retry",
    );
    failure = false;
    click("Acquire wave.gif");
    await until(() => !tests.querySelector('[aria-label="Acquire wave.gif"]'));
    check(
      actions.join(",") === "acquire,acquire" &&
        tests
          .querySelector(".library-owned-actions")
          ?.querySelectorAll("button").length === 1,
      "successful acquisition replaces the capsule with a single remove button",
    );
    item = { ...item, available: false, owned: false };
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => tests.textContent.includes("Media unavailable"));
    api.libraryChange = async (_id, action) => {
      actions.push(action);
      if (action === "remove") item = { ...item, saved: false };
      return { ok: true };
    };
    api.library = async () => page(item.saved ? [item] : []);
    click("Remove wave.gif");
    await until(() => !tests.querySelector('[aria-label="Remove wave.gif"]'));
    check(
      actions.at(-1) === "remove",
      "a retired borrowed entry can be removed to free its collection slot",
    );
  });
  await test("media library removal handles borrowers owners and final-owner races", async () => {
    let item = { ...sticker, owned: false };
    let recent = true;
    const actions = [];
    let race = false;
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async (view) =>
      page(
        (
          view === "owned"
            ? item.owned
            : view === "recent"
              ? recent
              : item.saved
        )
          ? [item]
          : [],
      );
    api.libraryChange = async (id, action, source, confirmed) => {
      actions.push({ id, action, source, confirmed });
      if (action === "release" && race && !confirmed)
        throw new ApiError(409, "Final ownership changed");
      if (action === "release") item = { ...item, owned: false };
      if (action === "remove") {
        item = { ...item, saved: false };
        recent = false;
      }
      return { ok: true };
    };
    render(
      h(LibraryPicker, {
        account: "remove-test",
        canSelect: true,
        select() {
          throw new Error("Removing must not select a sticker");
        },
        close() {},
      }),
      tests,
    );
    await manage();
    await until(() => tests.querySelector('[aria-label="Remove wave.gif"]'));
    click("Remove wave.gif");
    await until(() => !tests.querySelector('[aria-label="Remove wave.gif"]'));
    check(
      actions.length === 1 && actions[0].action === "remove",
      "borrowers remove directly without releasing ownership",
    );
    item = { ...sticker };
    recent = true;
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => tests.querySelector('[aria-label="Remove wave.gif"]'));
    click("Remove wave.gif");
    await until(() => !tests.querySelector('[aria-label="Remove wave.gif"]'));
    check(
      actions
        .slice(-2)
        .map((action) => action.action)
        .join(",") === "release,remove",
      "co-owners release their ownership and remove the entry",
    );
    item = { ...sticker, owner_count: 1 };
    recent = true;
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => tests.querySelector('[aria-label="Remove wave.gif"]'));
    const before = actions.length;
    click("Remove wave.gif");
    await until(() => tests.querySelector(".library-confirmation"));
    check(
      actions.length === before &&
        tests.textContent.includes("permanently removes it for everyone"),
      "known last owner sees global permanent removal warning before any mutation",
    );
    const popupBounds = tests
      .querySelector(".library-picker")
      .getBoundingClientRect();
    const confirmationBounds = tests
      .querySelector(".library-confirmation")
      .getBoundingClientRect();
    check(
      Math.abs(
        (confirmationBounds.top + confirmationBounds.bottom) / 2 -
          (popupBounds.top + popupBounds.bottom) / 2,
      ) < 2,
      "removal confirmation floats at the picker center",
    );
    check(
      tests.querySelector('[role="alertdialog"]') &&
        tests.querySelector(".library-content").inert,
      "confirmation is a modal dialog with inert sticker content",
    );
    const button = (name) =>
      [...tests.querySelectorAll("button")].find(
        (button) => button.textContent.trim() === name,
      );
    button("Keep sticker").click();
    await until(() => !tests.querySelector(".library-confirmation"));
    check(
      actions.length === before,
      "cancelling preserves ownership and the entry",
    );
    item = { ...sticker, filename: "race.gif" };
    race = true;
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(
      () =>
        tests.querySelector('[aria-label="Remove race.gif"]') &&
        !tests.querySelector('[aria-label="Remove race.gif"]').disabled,
    );
    click("Remove race.gif");
    await until(() => tests.querySelector(".library-confirmation"));
    check(
      actions.at(-1).action === "release" &&
        !actions.at(-1).confirmed &&
        item.saved,
      "last-owner race preserves the entry until confirmation",
    );
    button("Remove permanently").click();
    await until(() => !tests.querySelector('[aria-label="Remove race.gif"]'));
    check(
      actions.at(-2).confirmed === true && actions.at(-1).action === "remove",
      "confirmed removal explicitly releases final ownership then clears the entry",
    );
    render(h(StickerSettings), tests);
    await until(() => tests.querySelector(".library-quota"));
    check(
      !tests.querySelector("details, input, .library-management"),
      "Account settings only shows storage usage",
    );
    check(
      !tests
        .querySelector('[aria-label="Refresh media storage"]')
        .textContent.trim(),
      "storage refresh is an accessible icon-only button",
    );
  });

  await test("media library selection reuses an asset and cancellation preserves ownership", async () => {
    let allocations = 0;
    let abandonments = 0;
    let uploads;
    api.mediaConfig = async () => ({ enabled: true, max_size: 10000 });
    api.library = async () => page([sticker]);
    api.allocateMedia = async () => {
      allocations++;
      throw new Error("Unexpected upload");
    };
    api.abandonMedia = async () => {
      abandonments++;
      return { ok: true };
    };
    function Editor() {
      uploads = useMediaUploads("room-id");
      const input = useRef(null);
      return h(Composer, {
        mediaUpload: uploads,
        minimumComposerHeight: 40,
        actualComposerHeight: 40,
        hints: [],
        activeHint: 0,
        replyTarget: null,
        input,
        consoleView: false,
        draft: "",
        pending: false,
        canWrite: true,
        startResize() {},
        moveResize() {},
        endResize() {},
        resizeComposer() {},
        acceptHint() {},
        cancelReply() {},
        send(event) {
          event.preventDefault();
        },
        edit() {},
        keydown() {},
        selectCaret() {},
      });
    }
    render(h(Editor), tests);
    await until(() => uploads.enabled);
    tests.querySelector('[aria-label="Open media library"]').click();
    await until(() => tests.querySelector('[aria-label="Stickers"]'));
    click("Stickers");
    await until(() =>
      tests.querySelector(`[aria-label="Use ${sticker.filename}"]`),
    );
    tests.querySelector(`[aria-label="Use ${sticker.filename}"]`).click();
    await until(() => uploads.attachment && !tests.querySelector("dialog"));
    check(
      uploads.attachment.media.media_id === id && allocations === 0,
      "selection reuses stored bytes and ID",
    );
    click("Open media library");
    await until(() => tests.querySelector(".library-picker"));
    const reopened = tests.querySelector(".library-picker");
    check(
      document.activeElement === reopened,
      "reopening with queued media focuses the popup rather than a remove control",
    );
    await sleep(150);
    check(
      [...reopened.querySelectorAll(".library-sticker-actions")].every(
        (actions) => getComputedStyle(actions).opacity === "0",
      ),
      "reopening does not reveal unhovered sticker actions",
    );
    reopened.querySelector(".library-edit").click();
    await sleep(10);
    const remove = reopened.querySelector('[aria-label="Remove wave.gif"]');
    remove.focus();
    await sleep(150);
    check(
      getComputedStyle(remove.closest(".library-sticker-actions")).opacity ===
        "1",
      "keyboard focus still reveals the sticker actions",
    );
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    await until(() => !tests.querySelector(".library-picker"));
    tests.querySelector('[aria-label="Remove attachment"]').click();
    await until(() => !uploads.attachment);
    check(
      abandonments === 0,
      "removing an existing asset from the composer does not abandon storage",
    );
  });

  await test("media library older-message scrolling preserves pending attachment rows and collection controls", async () => {
    let waiting = false;
    const pending = [];
    const entries = Array.from({ length: 100 }, (_, index) => ({
      key: `media-row-${index}`,
      kind: "message",
      message: { sequence: index + 1 },
    }));
    const mediaInfo = {
      ...sticker,
      path: `/api/media/${id}/file`,
      library: true,
      saved: false,
      unavailable: false,
      expired: false,
      expires: null,
    };
    api.mediaStatus = () =>
      waiting
        ? new Promise((resolve) => pending.push(resolve))
        : Promise.resolve(mediaInfo);
    let timeline;
    function History() {
      const viewport = useRef(null);
      const handle = useRef(null);
      const atBottom = useRef(false);
      timeline = handle;
      const window = useVirtualTimeline(entries, viewport, handle, atBottom);
      return h(
        "section",
        {
          ref: viewport,
          class: "message-list",
          style:
            "flex:none;height:240px;width:360px;overflow-y:auto;overflow-anchor:none",
        },
        h(
          "div",
          { class: "timeline-rows" },
          h("div", { style: { height: window.top, flexShrink: 0 } }),
          ...window.visible.map((entry) =>
            h(
              "div",
              { key: entry.key, "data-virtual-key": entry.key },
              h(
                LibrarySourceContext.Provider,
                { value: "scroll-source" },
                h(
                  CollapsibleMessage,
                  { expanded: true, onExpandedChange() {} },
                  h(
                    ManagedAttachment,
                    { id },
                    h(
                      "span",
                      {
                        "data-media-body": true,
                        style: "display:block;height:120px",
                      },
                      "Media preview",
                    ),
                  ),
                ),
              ),
            ),
          ),
          h("div", { style: { height: window.bottom, flexShrink: 0 } }),
        ),
      );
    }
    render(
      h(
        ContentMotionContext.Provider,
        { value: { current: false } },
        h(History),
      ),
      tests,
    );
    await until(() =>
      tests.querySelector(
        '[data-virtual-key="media-row-0"] .library-save button',
      ),
    );
    await sleep(300);
    const height = tests
      .querySelector('[data-virtual-key="media-row-0"]')
      .getBoundingClientRect().height;
    timeline.current.scrollToSequence(60);
    await until(() => !tests.querySelector('[data-virtual-key="media-row-0"]'));
    await sleep(300);
    waiting = true;
    const viewport = tests.querySelector(".message-list");
    viewport.scrollTop = 100;
    viewport.dispatchEvent(new Event("scroll"));
    await until(
      () =>
        tests.querySelector(
          '[data-virtual-key="media-row-0"] [data-layout-pending]',
        ) && pending.length,
    );
    const row = tests.querySelector('[data-virtual-key="media-row-0"]');
    check(
      !row.querySelector("[data-media-body]"),
      "virtual remount rechecks access before displaying media",
    );
    check(
      Math.abs(row.getBoundingClientRect().height - height) < 1,
      "shared virtual timeline preserves measured height while attachment access checks are pending",
    );
    const top = viewport.scrollTop;
    for (const resolve of pending.splice(0))
      resolve({ ...mediaInfo, saved: true });
    await until(
      () => row.querySelector(".library-save button")?.textContent === "Saved",
    );
    for (let frame = 0; frame < 20; frame++) {
      await new Promise(requestAnimationFrame);
      check(
        Math.abs(row.getBoundingClientRect().height - height) < 1,
        "restored message must not replay its loading reveal on any animation frame",
      );
    }
    check(
      Math.abs(row.getBoundingClientRect().height - height) < 1 &&
        Math.abs(viewport.scrollTop - top) < 1,
      "resolving Saved state keeps the older-message scroll anchor and row height stable",
    );
    const preview = row.querySelector("[data-media-body]");
    const image = document.createElement("img");
    let imageReady = false;
    Object.defineProperty(image, "complete", { get: () => imageReady });
    preview.append(image);
    preview.style.height = "0px";
    await sleep(30);
    check(
      Math.abs(row.getBoundingClientRect().height - height) < 1,
      "the same shared row reservation covers delayed image loading after metadata resolves",
    );
    preview.style.height = "120px";
    imageReady = true;
    image.dispatchEvent(new Event("load"));
    await until(() => row.style.minHeight === "");
    check(
      row.style.minHeight === "" && Math.abs(viewport.scrollTop - top) < 1,
      "image completion releases the shared reservation without moving the reader",
    );
    waiting = false;
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(
      () =>
        row.querySelector(".library-save button")?.textContent ===
        "Add to collection",
    );
    await sleep(300);
    check(
      Math.abs(row.getBoundingClientRect().height - height) < 1 &&
        Math.abs(viewport.scrollTop - top) < 1,
      "collection status refreshes reuse the same control without shifting older messages",
    );
  });
  await test("media library message save uses source access and retirement replaces downloaded media", async () => {
    let info = {
      ...sticker,
      path: `/api/media/${id}/file`,
      library: true,
      saved: false,
      unavailable: false,
      expired: false,
      expires: null,
    };
    const calls = [];
    api.mediaStatus = async () => info;
    api.libraryChange = async (...args) => {
      calls.push(args);
      info = { ...info, saved: true };
      return { ok: true };
    };
    render(
      h(
        LibrarySourceContext.Provider,
        { value: "source-room" },
        h(
          ManagedAttachment,
          { id },
          h("span", { "data-image": true }, "downloaded image"),
        ),
      ),
      tests,
    );
    await until(() => tests.querySelector(".library-save button"));
    tests.querySelector(".library-save button").click();
    await until(() => tests.textContent.includes("Saved"));
    check(
      calls[0][0] === id &&
        calls[0][1] === "save" &&
        calls[0][2] === "source-room",
      "saving identifies the authorized source",
    );
    render(null, tests);
    render(
      h(
        LibrarySourceContext.Provider,
        { value: "source-room" },
        h(
          ManagedAttachment,
          { id },
          h("span", { "data-image": true }, "downloaded image"),
        ),
      ),
      tests,
    );
    await until(() => tests.textContent.includes("Saved"));
    check(
      tests.querySelector(".library-save button")?.disabled &&
        calls.length === 1,
      "remounting reads saved state from the server and does not offer duplicate collection addition",
    );
    info = { ...info, saved: false };
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(
      () =>
        tests.querySelector(".library-save button")?.textContent ===
        "Add to collection",
    );
    info = { ...info, unavailable: true };
    window.dispatchEvent(new Event("commonroom-library-change"));
    await until(() => tests.textContent.includes("Media unavailable"));
    check(
      !tests.querySelector("[data-image],.library-save"),
      "retirement removes already mounted media and save actions",
    );
    check(
      tests.textContent.includes(sticker.filename),
      "unavailability preserves the filename",
    );
    await sleep(10);
  });
}
