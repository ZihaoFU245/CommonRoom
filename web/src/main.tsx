import { disconnectPreferences } from "./features/preferences/account.ts";
import { loadInitialAccount } from "./api/bootstrap.ts";
import mediaWorkerUrl from "virtual:media-worker-url";
import { startMediaCache } from "./features/conversation/mediaController.ts";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "./api/protocol.ts";
import { errorMessage } from "./api/protocol.ts";
import { isUnauthorized } from "./api/client.ts";
import { Chat } from "./components/Chat.tsx";
import { Login } from "./components/Login.tsx";
import { bootAppearance } from "./features/preferences/appearance.ts";
import { useTheme } from "./hooks/useTheme.ts";
import "./style.css";
import { AppErrorBoundary } from "./components/AppErrorBoundary.tsx";
import { useAppViewport } from "./hooks/useAppViewport.ts";

/* Resolve the saved theme before the first render so it never paints the
   wrong background and then corrects itself. */
bootAppearance();

// Production HTTPS redirects and HSTS are configured by the serving proxy.
startMediaCache(new URL(mediaWorkerUrl, document.baseURI));
const root = document.getElementById("app");
if (!root) throw new Error("Missing application root.");
render(
  <AppErrorBoundary>
    <App />
  </AppErrorBoundary>,
  root,
);

function App() {
  useAppViewport();
  const appearance = useTheme();
  const [account, setAccount] = useState<Snapshot | null | undefined>(
    undefined,
  );
  const [initialError, setInitialError] = useState("");
  useEffect(() => {
    loadInitialAccount()
      .then(setAccount)
      .catch((error) => {
        if (!isUnauthorized(error)) setInitialError(errorMessage(error));
        setAccount(null);
      });
  }, []);
  if (account === undefined)
    return <div class="loading">Opening Commonroom…</div>;
  return account ? (
    <Chat
      initial={account}
      onLogout={() => {
        disconnectPreferences();
        setAccount(null);
      }}
      appearance={appearance}
    />
  ) : (
    <Login initialError={initialError} onLogin={setAccount} />
  );
}
