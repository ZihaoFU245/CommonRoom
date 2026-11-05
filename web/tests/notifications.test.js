import { test } from "node:test";
import assert from "node:assert/strict";
import {
  notifyMessages,
  toggleMessageNotifications,
} from "../src/features/preferences/notifications.ts";

test("muting never requests permission and enabling distinguishes granted, blocked and dismissed permission", async () => {
  let requests = 0;
  const notifications = {
    permission: "granted",
    async requestPermission() {
      requests++;
      return "granted";
    },
  };
  assert.equal(await toggleMessageNotifications(true, undefined, false), false);
  assert.equal(
    await toggleMessageNotifications(false, notifications, true),
    true,
  );
  assert.equal(requests, 0);
  notifications.permission = "default";
  assert.equal(
    await toggleMessageNotifications(false, notifications, true),
    true,
  );
  assert.equal(requests, 1);
  notifications.permission = "denied";
  await assert.rejects(
    toggleMessageNotifications(false, notifications, true),
    /blocked/,
  );
  assert.equal(requests, 1);
  notifications.permission = "default";
  notifications.requestPermission = async () => "default";
  await assert.rejects(
    toggleMessageNotifications(false, notifications, true),
    /remain muted/,
  );
  await assert.rejects(
    toggleMessageNotifications(false, undefined, true),
    /supported browser/,
  );
});
test("notification delivery respects permission, foreground view and reconnect baseline; click opens the conversation", () => {
  const previous = {
    username: "alice",
    rooms: [{ name: "room", messages: [] }],
    direct: [],
  };
  const next = {
    ...previous,
    rooms: [
      {
        name: "room",
        messages: [
          {
            id: "new",
            from: "bob",
            time: 10,
            text: "@alice " + "🙂".repeat(210),
            mentions: ["alice"],
          },
        ],
      },
    ],
  };
  const shown = [];
  class Notifications {
    static permission = "granted";
    constructor(title, options) {
      this.title = title;
      this.options = options;
      shown.push(this);
    }
    close() {
      this.closed = true;
    }
  }
  let selected;
  const options = {
    enabled: true,
    baseline: false,
    NotificationClass: Notifications,
    background: true,
    selected: "room",
    open: (view) => {
      selected = view;
    },
  };
  assert.equal(
    notifyMessages(previous, next, { ...options, baseline: true }),
    0,
  );
  assert.equal(
    notifyMessages(previous, next, { ...options, enabled: false }),
    0,
  );
  assert.equal(
    notifyMessages(previous, next, { ...options, background: false }),
    0,
  );
  Notifications.permission = "denied";
  assert.equal(notifyMessages(previous, next, options), 0);
  Notifications.permission = "granted";
  assert.equal(notifyMessages(previous, next, options), 1);
  assert.equal(shown[0].title, "#room");
  assert.equal(shown[0].options.tag, "new");
  const [mention, preview] = shown[0].options.body.split("\n");
  assert.equal(mention, "bob mentioned you");
  assert.equal(Array.from(preview).length, 200);
  assert.ok(preview.endsWith("🙂"));
  shown[0].onclick();
  assert.equal(selected, "room");
  assert.equal(shown[0].closed, true);
  assert.equal(notifyMessages(next, next, options), 0);
  assert.equal(
    notifyMessages(previous, next, {
      ...options,
      background: false,
      selected: "another",
    }),
    1,
  );
  class Unsupported extends Notifications {
    constructor() {
      throw new Error("unsupported");
    }
  }
  assert.equal(
    notifyMessages(previous, next, {
      ...options,
      NotificationClass: Unsupported,
    }),
    0,
  );
});

test("a DM without a mention alerts once and the bell mutes DMs and room mentions", async () => {
  const previous = {
    username: "alice",
    rooms: [{ name: "room", messages: [] }],
    direct: [],
    unread: {},
  };
  const dm = {
    id: "dm",
    from: "bob",
    to: "alice",
    sequence: 1,
    text: "hello",
    mentions: [],
  };
  const mention = {
    ...dm,
    id: "mention",
    to: null,
    time: 2,
    sequence: 2,
    text: "@alice",
    mentions: ["alice"],
  };
  const next = {
    ...previous,
    direct: [dm],
    rooms: [{ name: "room", messages: [mention] }],
  };
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
    baseline: false,
    NotificationClass: Notifications,
    background: false,
    selected: "@direct:bob",
    open: (view) => {
      opened = view;
    },
  };
  assert.equal(notifyMessages(previous, next, options), 1);
  assert.equal(shown[0].title, "#room");
  assert.equal(shown[0].body, "bob mentioned you\n@alice");
  assert.equal(
    notifyMessages(previous, next, { ...options, selected: "room" }),
    1,
  );
  assert.equal(shown[1].title, "bob");
  assert.equal(shown[1].body, "hello");
  shown[1].notification.onclick();
  assert.equal(opened, "@direct:bob");
  const enabled = await toggleMessageNotifications(true, Notifications, true);
  assert.equal(
    notifyMessages(previous, next, { ...options, enabled, background: true }),
    0,
  );
  assert.equal(shown.length, 2);
});
