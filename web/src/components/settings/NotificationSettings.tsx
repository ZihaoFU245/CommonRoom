import { useEffect, useRef, useState } from "preact/hooks";
import { useSyncExternalStore } from "preact/compat";
import { api } from "../../api/client.ts";
import { errorMessage } from "../../api/protocol.ts";
import { ensureMediaCache } from "../../features/conversation/mediaController.ts";
import {
  saveAccountPreferences,
  savedPreferences,
  subscribePreferences,
} from "../../features/preferences/account.ts";
import {
  globalNotificationFilter,
  notificationDefaultOptions,
  notificationScopes,
  type NotificationScopes,
} from "../../features/preferences/notificationDefaults.ts";
import { ScopePicker } from "./ScopePicker.tsx";

interface PushState {
  key: Uint8Array<ArrayBuffer> | null;
  manager: PushManager | null;
  installed: boolean;
  subscription: PushSubscription | null;
}

function matchesKey(
  subscription: PushSubscription,
  key: Uint8Array<ArrayBuffer> | null,
) {
  const previous = subscription.options.applicationServerKey;
  if (!previous || !key || previous.byteLength !== key.byteLength) return false;
  return new Uint8Array(previous).every((value, index) => value === key[index]);
}

const emptyPreferences = {};
const globalScope = { id: null, label: "Global", direct: false };

function NotificationDefaults({ context }: { context: NotificationScopes }) {
  const preferences = useSyncExternalStore(subscribePreferences, () =>
    savedPreferences(context.preferences ?? emptyPreferences),
  );
  const [selected, selectScope] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const writing = useRef(false);
  const [error, setError] = useState("");
  const scopes = [globalScope, ...notificationScopes(context)];
  const scope = scopes.find(({ id }) => id === selected) ?? globalScope;
  const customScope =
    scope.id !== null && preferences[`notification.${scope.id}`] !== undefined;
  const customGlobal = preferences[globalNotificationFilter] !== undefined;
  const overrides =
    scope.id === null
      ? []
      : notificationDefaultOptions
          .map(({ key }) => `${key}.${scope.id}`)
          .filter((key) => preferences[key] !== undefined);
  const options = notificationDefaultOptions.filter(
    ({ key }) =>
      scope.id === null ||
      (scope.direct
        ? key !== "notification_default_mentions_all"
        : key !== "notification_default_direct"),
  );

  async function save(values: Record<string, string | null>) {
    if (writing.current) return;
    writing.current = true;
    setSaving(true);
    setError("");
    try {
      await saveAccountPreferences(() => api.preferences(values, false));
    } finally {
      writing.current = false;
      setSaving(false);
    }
  }

  return (
    <>
      <h3>Notification filters</h3>
      <div class="name-color-setting name-color-scope">
        <span id="notification-scope-label">Scope</span>
        <ScopePicker
          id="notification-scope"
          label="Notification scope"
          labelId="notification-scope-label"
          scopes={scopes}
          selected={scope}
          saving={false}
          onChoose={(id) => {
            setError("");
            selectScope(id);
          }}
        />
      </div>
      <p class="account-summary">
        {customScope || customGlobal ? (
          <>
            A custom {customScope ? "conversation" : "global"} filter applies.
            Clear it with{" "}
            <code>
              /notifications filter default{" "}
              {customScope ? scope.label : "global"}
            </code>{" "}
            to use these defaults.
          </>
        ) : scope.id === null ? (
          "Global defaults apply to conversations that inherit them."
        ) : overrides.length ? (
          "Changed options apply to this conversation. Other options follow Global."
        ) : (
          "This conversation follows Global until you change an option."
        )}
      </p>
      {options.map(({ key, label }) => {
        const setting = scope.id === null ? key : `${key}.${scope.id}`;
        const checked = (preferences[setting] ?? preferences[key]) !== "false";
        const id = `settings-${key}`;
        return (
          <label class="security-choice notification-choice" for={id} key={key}>
            <input
              id={id}
              type="checkbox"
              checked={checked}
              disabled={saving || customScope || customGlobal}
              onChange={(event) => {
                const next = event.currentTarget.checked;
                event.currentTarget.checked = checked;
                save({ [setting]: String(next) }).catch((cause: unknown) =>
                  setError(errorMessage(cause)),
                );
              }}
            />
            <span>{label}</span>
          </label>
        );
      })}
      {overrides.length > 0 && (
        <button
          type="button"
          class="displayname-record-remove notification-restore"
          disabled={saving}
          onClick={() => {
            save(Object.fromEntries(overrides.map((key) => [key, null]))).catch(
              (cause: unknown) => setError(errorMessage(cause)),
            );
          }}
        >
          Use global defaults
        </button>
      )}
      {error && (
        <p class="account-error" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

export function NotificationSettings({
  context,
}: {
  context: NotificationScopes;
}) {
  const [state, setState] = useState<PushState>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"install" | "uninstall">();
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      const config = await api.pushSettings(controller.signal);
      const supported =
        window.isSecureContext &&
        "serviceWorker" in navigator &&
        typeof PushManager !== "undefined" &&
        typeof Notification !== "undefined";
      let manager: PushManager | null = null;
      if (supported && (config.public_key || config.installed)) {
        await ensureMediaCache();
        const registration = await navigator.serviceWorker.ready;
        manager = registration.pushManager;
      }
      const subscription = await manager?.getSubscription();
      if (!controller.signal.aborted)
        setState({
          key: config.public_key
            ? Uint8Array.from(
                atob(config.public_key.replace(/-/gu, "+").replace(/_/gu, "/")),
                (byte) => byte.charCodeAt(0),
              )
            : null,
          manager,
          installed: config.installed,
          subscription: subscription ?? null,
        });
    }
    load().catch((cause: unknown) => {
      if (!controller.signal.aborted) setError(errorMessage(cause));
    });
    return () => controller.abort();
  }, []);

  async function install() {
    if (!state?.manager || !state.key || state.subscription || busy) return;
    setBusy("install");
    setError("");
    try {
      // Safari requires subscribe() to run directly from the Install click.
      const subscription = await state.manager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: state.key,
      });
      setState({ ...state, subscription });
      await api.installPush(subscription.toJSON());
      setState({ ...state, installed: true, subscription });
    } finally {
      setBusy(undefined);
    }
  }

  async function uninstall() {
    if (!state || busy) return;
    setBusy("uninstall");
    setError("");
    try {
      if (state.installed) await api.uninstallPush();
      setState({ ...state, installed: false });
      const subscription = await state.manager?.getSubscription();
      if (subscription && !(await subscription.unsubscribe())) {
        const remaining = await state.manager?.getSubscription();
        if (remaining)
          throw new Error(
            "Could not remove the browser subscription. Try again.",
          );
      }
      setState({ ...state, installed: false, subscription: null });
    } finally {
      setBusy(undefined);
    }
  }

  const incompatible = state?.subscription
    ? !!state.key && !matchesKey(state.subscription, state.key)
    : false;
  const repair =
    incompatible || (state && state.installed !== !!state.subscription);
  const remove =
    busy === "uninstall" || state?.installed || !!state?.subscription;
  const issue =
    error ||
    (repair
      ? "Uninstall the previous subscription, then install again."
      : state && !state.key
        ? "Background notifications are disabled on this server."
        : state && !state.manager
          ? "On iPhone or iPad, use Safari’s Share menu to add CommonRoom to the Home Screen as a web app. Open it from the Home Screen, then return here to enable notifications."
          : "");
  return (
    <>
      <div class="notification-device">
        <h3>Background notifications</h3>
        <button
          type="button"
          class={`notification-device-action ${remove ? "account-setting-action" : "primary"}`}
          aria-busy={!!busy || (!state && !error)}
          disabled={!!busy || (!remove && (!state?.key || !state.manager))}
          onClick={() => {
            (remove ? uninstall() : install()).catch((cause: unknown) =>
              setError(errorMessage(cause)),
            );
          }}
        >
          {remove ? "Uninstall" : "Install"}
        </button>
        <p class="account-summary">Receives background notification pushes.</p>
      </div>
      {issue && (
        <p role="status" class="account-summary">
          {issue}
        </p>
      )}
      <NotificationDefaults context={context} />
    </>
  );
}
