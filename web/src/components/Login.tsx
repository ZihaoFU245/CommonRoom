import { loadInitialAccount } from "../api/bootstrap.ts";
import type { JSX } from "preact";
import { useState } from "preact/hooks";
import type { Snapshot } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { Brand } from "./Brand.tsx";
import { Register } from "./Register.tsx";
import { LoginServerSettings } from "./LoginServerSettings.tsx";

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
  const [registering, setRegistering] = useState(false);
  const [registered, setRegistered] = useState(false);
  const [registrationKey, setRegistrationKey] = useState(0);
  async function submit(event: JSX.TargetedSubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setRegistered(false);
    setError("");
    try {
      await api.login(username, password);
      onLogin(await loadInitialAccount());
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
      {registering ? (
        <Register
          key={registrationKey}
          busy={busy}
          onBusy={setBusy}
          onCancel={() => setRegistering(false)}
          onRegistered={(name) => {
            setUsername(name);
            setRegistered(true);
            setRegistering(false);
          }}
        />
      ) : (
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
              maxLength={64}
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
            {registered && (
              <p role="status">Account created. You can now sign in.</p>
            )}
            {error && (
              <div class="form-error" role="alert">
                {error}
              </div>
            )}
            <button class="primary" disabled={busy}>
              {busy ? "Signing in…" : "Sign in"}
              <span aria-hidden="true">↗</span>
            </button>
            <button
              class="login-switch"
              type="button"
              disabled={busy}
              onClick={() => {
                setPassword("");
                setError("");
                setRegistered(false);
                setRegistering(true);
              }}
            >
              Register
            </button>
          </form>
        </section>
      )}
      <LoginServerSettings
        disabled={busy}
        onSave={() => {
          setPassword("");
          setError("");
          setRegistered(false);
          setRegistrationKey((key) => key + 1);
        }}
      />
    </main>
  );
}
