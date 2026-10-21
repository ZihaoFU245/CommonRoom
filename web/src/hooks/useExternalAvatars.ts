import { useEffect, useState } from "preact/hooks";
const key = "commonroom-external-avatars";
function read(): boolean {
  try {
    return localStorage.getItem(key) === "true";
  } catch {
    return false;
  }
}
export function useExternalAvatars(): [boolean, (enabled: boolean) => void] {
  const [enabled, setEnabled] = useState(read);
  useEffect(() => {
    const update = () => setEnabled(read());
    window.addEventListener("storage", update);
    window.addEventListener(key, update);
    return () => {
      window.removeEventListener("storage", update);
      window.removeEventListener(key, update);
    };
  }, []);
  return [
    enabled,
    (value) => {
      try {
        localStorage.setItem(key, String(value));
      } catch {
        /* Keep the safe default if storage is disabled. */
      }
      window.dispatchEvent(new Event(key));
    },
  ];
}
