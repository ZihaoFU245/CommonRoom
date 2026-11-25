import { useRef, useState } from "preact/hooks";
import { useSyncExternalStore } from "preact/compat";
import type { Snapshot } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import {
  savedPreferences,
  saveAccountPreferences,
  subscribePreferences,
} from "../features/preferences/account.ts";
import { focusControl } from "../features/preferences/inputFocus.ts";
import {
  defaultMatchers,
  globalNotificationFilter as globalFilter,
  notificationDefaultOptions,
} from "../features/preferences/notificationDefaults.ts";

export type NotificationStatusContext = Pick<
  Snapshot,
  "rooms" | "private_access" | "preferences"
>;

const emptyPreferences = {};
const defaultLabels = new Map<string, string>(
  notificationDefaultOptions.map(({ key, label }) => [key, label]),
);

export function NotificationStatus({
  context,
}: {
  context: NotificationStatusContext;
}) {
  const preferences = useSyncExternalStore(subscribePreferences, () =>
    savedPreferences(context.preferences ?? emptyPreferences),
  );
  const [busy, setBusy] = useState<string>();
  const saving = useRef(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const [feedback, setFeedback] = useState("");
  const [error, setError] = useState("");
  const names = new Map([
    ...context.rooms.map(({ id, name }) => [id, `#${name}`] as const),
    ...Object.entries(context.private_access).map(
      ([view, { id }]) => [id, view] as const,
    ),
  ]);
  const overrides = Object.entries(preferences)
    .flatMap(([key, value]) => {
      const separator = key.indexOf(".");
      if (separator < 0 || key === globalFilter) return [];
      const prefix = key.slice(0, separator);
      const label = defaultLabels.get(prefix);
      if (prefix !== "notification" && label === undefined) return [];
      const id = key.slice(separator + 1);
      const scope = names.get(id) ?? `Unavailable conversation (${id})`;
      return [
        {
          key,
          rule: label ? `${label}: ${value === "true" ? "On" : "Off"}` : value,
          scope,
          code: label === undefined,
          action: label
            ? `Delete ${label} override for ${scope}`
            : `Delete notification filter for ${scope}`,
          removeValue: null,
        },
      ];
    })
    .sort((a, b) => a.scope.localeCompare(b.scope));
  const globalRule = preferences[globalFilter];
  const globalRows =
    globalRule === undefined
      ? defaultMatchers
          .filter(({ key }) => preferences[key] !== "false")
          .map(({ key, label }) => ({
            key,
            scope: "Global",
            rule: label,
            code: false,
            action: `Delete ${label} from Global`,
            removeValue: "false",
          }))
      : [
          {
            key: globalFilter,
            scope: "Global",
            rule: globalRule,
            code: true,
            action: "Delete notification filter for Global",
            removeValue: null,
          },
        ];
  const rows = [...globalRows, ...overrides];
  const muted =
    preferences.notifications_muted === undefined
      ? preferences.notifications === "false"
      : preferences.notifications_muted === "true";
  const excludeBots = preferences.notification_exclude_bots !== "false";
  const excludeMentions =
    preferences.notification_exclude_bot_mentions !== "false";
  const defaultReplies = preferences.notification_default_replies !== "false";
  const hasCustomFilters =
    globalRule !== undefined ||
    overrides.length > 0 ||
    defaultMatchers.some(({ key }) => preferences[key] === "false") ||
    !excludeBots ||
    !excludeMentions;

  async function update(
    key: string | null,
    value: string | null,
    trigger: HTMLButtonElement,
  ) {
    if (saving.current) return;
    saving.current = true;
    setBusy(key ?? "reset");
    setFeedback("");
    setError("");
    const restoreFocus = trigger === document.activeElement;
    try {
      await saveAccountPreferences(() =>
        key === null
          ? api.resetNotificationFilters()
          : api.preferences({ [key]: value }, false),
      );
      if (
        restoreFocus &&
        (document.activeElement === trigger ||
          document.activeElement === document.body)
      )
        focusControl(heading.current, { preventScroll: true });
      setFeedback(
        key === null
          ? "Notification filters reset."
          : "Notification filter deleted.",
      );
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(undefined);
      saving.current = false;
    }
  }

  return (
    <section class="manual notification-status">
      <h2 ref={heading} tabIndex={-1}>
        Notification status
      </h2>
      <p>
        Alerts are <strong>{muted ? "muted" : "unmuted"}</strong>.
      </p>
      <p>
        By default globally, unthreaded bot messages{" "}
        {excludeBots ? "are filtered" : "can alert"}. Replies from bots{" "}
        {defaultReplies
          ? "alert only the person they reply to"
          : "are filtered"}
        . Messages mentioning bots{" "}
        {excludeMentions ? "are filtered" : "can alert"}.
      </p>
      <div
        class="notification-status-scroll"
        role="region"
        aria-label="Notification filter table"
        tabIndex={0}
      >
        <table aria-label="Notification filters">
          <thead>
            <tr>
              <th scope="col">Scope</th>
              <th scope="col">Filter</th>
              <th scope="col">Actions</th>
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 && (
              <tr>
                <td colSpan={3}>No notification filters.</td>
              </tr>
            )}
            {rows.map(({ key, scope, rule, code, action, removeValue }) => (
              <tr key={key}>
                <th scope="row">{scope}</th>
                <td>{code ? <code>{rule}</code> : rule}</td>
                <td>
                  <button
                    type="button"
                    class="displayname-record-remove"
                    aria-label={action}
                    aria-busy={busy === key}
                    disabled={busy !== undefined}
                    onClick={(event) => {
                      update(key, removeValue, event.currentTarget).catch(
                        (cause: unknown) => setError(errorMessage(cause)),
                      );
                    }}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <button
        type="button"
        class="displayname-record-remove"
        aria-busy={busy === "reset"}
        disabled={busy !== undefined || !hasCustomFilters}
        onClick={(event) => {
          update(null, null, event.currentTarget).catch((cause: unknown) =>
            setError(errorMessage(cause)),
          );
        }}
      >
        Reset to default
      </button>
      {feedback && <p role="status">{feedback}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
