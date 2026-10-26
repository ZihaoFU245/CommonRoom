import { useState } from "preact/hooks";
import { avatarSource } from "../features/preferences/avatar.ts";
import { useExternalAvatars } from "../hooks/useExternalAvatars.ts";
export function Avatar({
  url,
  name,
  large = false,
  fallback = false,
  decorative = true,
  preview,
}: {
  url: string | undefined;
  name: string;
  large?: boolean;
  fallback?: boolean;
  decorative?: boolean;
  preview?: string;
}) {
  const [external] = useExternalAvatars();
  const source = preview ?? avatarSource(url, document.baseURI, external);
  const [shown, setShown] = useState<{
    source: string;
    name: string;
    external: boolean;
  }>();
  const [failed, setFailed] = useState<string>();
  const painted =
    source && shown?.name === name && (external || !shown.external)
      ? shown.source
      : undefined;
  const placeholder = fallback ? <span aria-hidden="true">+</span> : null;
  if (!source || (!painted && failed === source)) return placeholder;
  const images =
    painted === source
      ? [source]
      : [painted, failed !== source ? source : undefined];
  return (
    <>
      {!painted && placeholder}
      {images.map(
        (image) =>
          image && (
            <img
              // A rename must reload the reused URL so onLoad reveals the new identity.
              key={`${name}:${image}`}
              class={`user-avatar${large ? " large" : ""}`}
              src={image}
              alt={decorative ? "" : `${name}'s avatar`}
              aria-hidden={decorative ? "true" : undefined}
              width={large ? 72 : 28}
              height={large ? 72 : 28}
              hidden={image !== painted}
              decoding="async"
              referrerPolicy="no-referrer"
              onLoad={() => {
                if (image === source)
                  setShown({
                    source,
                    name,
                    external: !preview && !!url?.startsWith("https:"),
                  });
              }}
              onError={() => setFailed(image)}
            />
          ),
      )}
    </>
  );
}
