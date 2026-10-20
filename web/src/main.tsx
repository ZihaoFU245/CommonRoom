import mediaWorkerUrl from "virtual:media-worker-url";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "./api/protocol.ts";
import { errorMessage } from "./api/protocol.ts";
import { api, isUnauthorized } from "./api/client.ts";
import { Chat } from "./components/Chat.tsx";
import { Login } from "./components/Login.tsx";
import { bootAppearance } from "./features/preferences/appearance.ts";
import { useTheme } from "./hooks/useTheme.ts";
import "./style.css";

/* Resolve the saved theme before the first render so it never paints the
   wrong background and then corrects itself. */
bootAppearance();

if (import.meta.env.PROD && location.protocol !== "https:") {
  location.replace(
    `https://${location.host}${location.pathname}${location.search}${location.hash}`,
  );
} else {
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker
      .register(new URL(mediaWorkerUrl, document.baseURI), {
        type: "module",
        scope: new URL("./", document.baseURI).pathname,
        updateViaCache: "none",
      })
      .catch(() => {
        /* Browsers without worker storage use ordinary downloads. */
      });
  }
  const root = document.getElementById("app");
  if (!root) throw new Error("Missing application root.");
  render(<App />, root);
}

function App() {
  const appearance = useTheme();
  const [account, setAccount] = useState<Snapshot | null | undefined>(
    undefined,
  );
  const [initialError, setInitialError] = useState("");
  useEffect(() => {
    api
      .me()
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
      onLogout={() => setAccount(null)}
      appearance={appearance}
    />
  ) : (
    <Login initialError={initialError} onLogin={setAccount} />
  );
}
