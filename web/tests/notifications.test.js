import { test } from "node:test";
import assert from "node:assert/strict";
import {
  notifyMentions,
  toggleMentionNotifications,
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
  assert.equal(await toggleMentionNotifications(true, undefined, false), false);
  assert.equal(
    await toggleMentionNotifications(false, notifications, true),
    true,
  );
  assert.equal(requests, 0);
  notifications.permission = "default";
  assert.equal(
    await toggleMentionNotifications(false, notifications, true),
    true,
  );
  assert.equal(requests, 1);
  notifications.permission = "denied";
  await assert.rejects(
    toggleMentionNotifications(false, notifications, true),
    /blocked/,
  );
  assert.equal(requests, 1);
  notifications.permission = "default";
  notifications.requestPermission = async () => "default";
  await assert.rejects(
    toggleMentionNotifications(false, notifications, true),
    /remain muted/,
  );
  await assert.rejects(
    toggleMentionNotifications(false, undefined, true),
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
    notifyMentions(previous, next, { ...options, baseline: true }),
    0,
  );
  assert.equal(
    notifyMentions(previous, next, { ...options, enabled: false }),
    0,
  );
  assert.equal(
    notifyMentions(previous, next, { ...options, background: false }),
    0,
  );
  Notifications.permission = "denied";
  assert.equal(notifyMentions(previous, next, options), 0);
  Notifications.permission = "granted";
  assert.equal(notifyMentions(previous, next, options), 1);
  assert.equal(shown[0].options.tag, "new");
  assert.equal(Array.from(shown[0].options.body).length, 200);
  assert.ok(shown[0].options.body.endsWith("🙂"));
  shown[0].onclick();
  assert.equal(selected, "room");
  assert.equal(shown[0].closed, true);
  assert.equal(notifyMentions(next, next, options), 0);
  assert.equal(
    notifyMentions(previous, next, {
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
    notifyMentions(previous, next, {
      ...options,
      NotificationClass: Unsupported,
    }),
    0,
  );
});
