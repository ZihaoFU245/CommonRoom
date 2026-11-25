import { test } from "node:test";
import assert from "node:assert/strict";
import { parseFrame } from "../src/api/protocol.ts";
import { showChatNotification } from "../src/features/preferences/notifications.ts";
import { isPushPayload } from "../src/features/preferences/pushPayload.ts";

const notice = {
  kind: "notification",
  id: "message",
  view: "lobby",
  title: "#lobby",
  body: "hello",
};

test("server notification frames are validated and respect local presentation state", () => {
  assert.deepEqual(parseFrame(JSON.stringify(notice)), notice);
  assert.equal(
    parseFrame(JSON.stringify({ ...notice, body: "x".repeat(402) })),
    null,
  );
  const shown = [];
  class Notifications {
    static permission = "granted";
    constructor(title, options) {
      shown.push({ title, ...options, notification: this });
    }
    close() {}
  }
  let opened;
  const options = {
    enabled: true,
    NotificationClass: Notifications,
    background: false,
    selected: "lobby",
    open: (view) => {
      opened = view;
    },
  };
  showChatNotification(notice, options);
  showChatNotification(notice, {
    ...options,
    background: true,
    enabled: false,
  });
  assert.equal(shown.length, 0);
  showChatNotification(notice, { ...options, background: true });
  assert.equal(shown.length, 1);
  assert.equal(shown[0].body, "hello");
  shown[0].notification.onclick();
  assert.equal(opened, "lobby");
});

test("the longest server preview passes validation on both notification paths", () => {
  const frame = { ...notice, body: `${"🦀".repeat(200)}…` };
  assert.deepEqual(parseFrame(JSON.stringify(frame)), frame);
  assert.equal(isPushPayload(frame), true);
  assert.equal(isPushPayload({ ...frame, body: frame.body + "!" }), false);
});

test("platforms that require persistent notifications can display the same server frame", async () => {
  class Notifications {
    static permission = "granted";
    constructor() {
      throw new TypeError("Persistent notifications required");
    }
  }
  const shown = [];
  showChatNotification(notice, {
    enabled: true,
    NotificationClass: Notifications,
    background: true,
    selected: "lobby",
    open: () => {},
    worker: Promise.resolve({
      showNotification: (title, options) => shown.push({ title, ...options }),
    }),
  });
  await Promise.resolve();
  assert.deepEqual(shown[0].data, { id: notice.id, view: notice.view });
});
