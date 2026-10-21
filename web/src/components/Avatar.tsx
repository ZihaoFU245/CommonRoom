import { useState } from "preact/hooks";
import { avatarSource } from "../features/preferences/avatar.ts";
export function Avatar({
  url,
  name,
  large = false,
  fallback = false,
}: {
  url: string | undefined;
  name: string;
  large?: boolean;
  fallback?: boolean;
}) {
  const source = avatarSource(url, document.baseURI);
  const [loaded, setLoaded] = useState<string>();
  const [failed, setFailed] = useState<string>();
  const placeholder = fallback ? <span aria-hidden="true">+</span> : null;
  if (!source || failed === source) return placeholder;
  return (
    <>
      {loaded !== source && placeholder}
      <img
        class={`user-avatar${large ? " large" : ""}`}
        src={source}
        alt={`${name}'s avatar`}
        hidden={loaded !== source}
        decoding="async"
        referrerPolicy="no-referrer"
        onLoad={() => setLoaded(source)}
        onError={() => setFailed(source)}
      />
    </>
  );
}
