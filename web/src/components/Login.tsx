import type { JSX } from "preact";
import { useState } from "preact/hooks";
import type { Snapshot } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { Brand } from "./Brand.tsx";

export function Login({
  onLogin,
  initialError,
}: {
  onLogin: (account: Snapshot) => void;
  initialError: string;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState(initialError);
  const [busy, setBusy] = useState(false);
  async function submit(event: JSX.TargetedSubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api.login(username, password);
      onLogin(await api.me());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <main class="login-page">
      <div class="login-top">
        <Brand />
      </div>
      <section class="login-card">
        <h1>Sign in.</h1>
        <form
          onSubmit={(event) => {
            submit(event).catch((error: unknown) =>
              setError(errorMessage(error)),
            );
          }}
        >
          <label for="username">Username</label>
          <input
            id="username"
            autoComplete="username"
            autoFocus
            maxLength={32}
            value={username}
            onInput={(e) => setUsername(e.currentTarget.value)}
            required
            placeholder="Your username"
          />
          <label for="password">Password</label>
          <input
            id="password"
            type="password"
            autoComplete="current-password"
            maxLength={128}
            value={password}
            onInput={(e) => setPassword(e.currentTarget.value)}
            required
            placeholder="Your password"
          />
          {error && (
            <div class="form-error" role="alert">
              {error}
            </div>
          )}
          <button class="primary" disabled={busy}>
            {busy ? "Signing in…" : "Sign in"}
            <span aria-hidden="true">↗</span>
          </button>
        </form>
      </section>
    </main>
  );
}
