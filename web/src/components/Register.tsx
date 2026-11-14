import type { JSX } from "preact";
import { useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";

export function Register({
  busy,
  onBusy,
  onCancel,
  onRegistered,
}: {
  busy: boolean;
  onBusy: (busy: boolean) => void;
  onCancel: () => void;
  onRegistered: (username: string) => void;
}) {
  const [error, setError] = useState("");

  async function submit(event: JSX.TargetedSubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    const fields = new FormData(event.currentTarget);
    const username = fields.get("username");
    const password = fields.get("password");
    const invite = fields.get("invite");
    if (
      typeof username !== "string" ||
      typeof password !== "string" ||
      typeof invite !== "string"
    ) {
      setError("Enter an invite code, username, and password.");
      return;
    }
    if (password !== fields.get("confirmation")) {
      setError("Passwords do not match.");
      return;
    }

    onBusy(true);
    setError("");
    try {
      await api.register(username, password, invite);
      onRegistered(username);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      onBusy(false);
    }
  }

  return (
    <section class="login-card">
      <h1>Create account.</h1>
      <form
        onSubmit={(event) => {
          submit(event).catch((e: unknown) => setError(errorMessage(e)));
        }}
      >
        <label for="register-invite">Invite code</label>
        <input
          id="register-invite"
          name="invite"
          autoComplete="off"
          autoCapitalize="off"
          spellcheck={false}
          autoFocus
          maxLength={128}
          required
          readOnly={busy}
        />
        <label for="register-username">Username</label>
        <input
          id="register-username"
          name="username"
          autoComplete="username"
          autoCapitalize="off"
          maxLength={64}
          required
          readOnly={busy}
        />
        <label for="register-password">Password</label>
        <input
          id="register-password"
          name="password"
          type="password"
          autoComplete="new-password"
          minLength={3}
          maxLength={128}
          required
          readOnly={busy}
        />
        <label for="register-confirmation">Confirm password</label>
        <input
          id="register-confirmation"
          name="confirmation"
          type="password"
          autoComplete="new-password"
          maxLength={128}
          required
          readOnly={busy}
        />
        {error && (
          <div class="form-error" role="alert">
            {error}
          </div>
        )}
        <button class="primary" disabled={busy}>
          {busy ? "Creating account…" : "Create account"}
          <span aria-hidden="true">↗</span>
        </button>
        <button
          class="login-switch"
          type="button"
          disabled={busy}
          onClick={onCancel}
        >
          Back to sign in
        </button>
      </form>
    </section>
  );
}
