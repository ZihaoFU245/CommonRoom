import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "./api/protocol.ts";
import { errorMessage } from "./api/protocol.ts";
import { api } from "./api/client.ts";
import { Chat } from "./components/Chat.tsx";
import { Login } from "./components/Login.tsx";
import "./style.css";

if (import.meta.env.PROD && location.protocol !== "https:") {
  location.replace(
    `https://${location.host}${location.pathname}${location.search}${location.hash}`,
  );
} else {
  const root = document.getElementById("app");
  if (!root) throw new Error("Missing application root.");
  render(<App />, root);
}

function App() {
  const [account, setAccount] = useState<Snapshot | null | undefined>(
    undefined,
  );
  const [initialError, setInitialError] = useState("");
  useEffect(() => {
    api
      .me()
      .then(setAccount)
      .catch((error) => {
        if (
          errorMessage(error) !== "Please log in." &&
          errorMessage(error) !== "Account unavailable."
        )
          setInitialError(errorMessage(error));
        setAccount(null);
      });
  }, []);
  if (account === undefined)
    return <div class="loading">Opening Commonroom…</div>;
  return account ? (
    <Chat initial={account} onLogout={() => setAccount(null)} />
  ) : (
    <Login initialError={initialError} onLogin={setAccount} />
  );
}
