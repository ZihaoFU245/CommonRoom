import { stopClientMetrics, recordClientMetric } from "./api/clientMetrics.ts";
import { metric } from "./api/clientMetricSchema.ts";
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
import { useTheme } from "./hooks/useTheme.ts";
import "./style.css";
import { AppErrorBoundary } from "./components/AppErrorBoundary.tsx";
import { useAppViewport } from "./hooks/useAppViewport.ts";
import { trackFocusInput } from "./features/preferences/inputFocus.ts";

// Chrome on iOS can also expose WebKit CSS features.
if (
  CSS.supports("-webkit-touch-callout", "none") &&
  !/(?:CriOS|FxiOS|EdgiOS|OPiOS)\//u.test(navigator.userAgent) &&
  (/Version\/.*Safari\//u.test(navigator.userAgent) ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true ||
    matchMedia("(display-mode: standalone)").matches)
) {
  document.documentElement.dataset.iosSafari = "";
}

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
  useEffect(trackFocusInput, []);
  const appearance = useTheme();
  const [account, setAccount] = useState<Snapshot | null | undefined>(
    undefined,
  );
  const [initialError, setInitialError] = useState("");
  useEffect(() => {
    loadInitialAccount()
      .then(setAccount)
      .catch((error) => {
        if (!isUnauthorized(error)) {
          recordClientMetric(metric.bootstrapError);
          setInitialError(errorMessage(error));
        }
        setAccount(null);
      });
  }, []);
  if (account === undefined)
    return <div class="loading">Opening Commonroom…</div>;
  return account ? (
    <Chat
      initial={account}
      onLogout={() => {
        stopClientMetrics();
        disconnectPreferences();
        setAccount(null);
      }}
      appearance={appearance}
    />
  ) : (
    <Login initialError={initialError} onLogin={setAccount} />
  );
}
