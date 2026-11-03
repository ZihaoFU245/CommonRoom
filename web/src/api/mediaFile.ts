/** Shared managed-file identity and signed-link validation, also used by the worker. */
export function mediaFilePath(path: string): {
  kind: "media" | "avatars";
  id: string;
  basePath: string;
} | null {
  const match =
    /^(\/(?:[a-z0-9_-]+\/)*)api\/(media|avatars)\/([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})\/file$/iu.exec(
      path,
    );
  const kind = match?.[2];
  return match?.[1] && match[3] && (kind === "media" || kind === "avatars")
    ? { kind, id: match[3], basePath: match[1] }
    : null;
}

export function mediaFileUrl(href: string, base?: string) {
  try {
    const url = new URL(href, base);
    const path = mediaFilePath(url.pathname);
    return path &&
      /^https?:$/u.test(url.protocol) &&
      !url.username &&
      !url.password &&
      !url.hash
      ? { ...path, url, base: new URL(path.basePath, url).href }
      : null;
  } catch {
    return null;
  }
}

/** Keep download navigations inside the UI worker's scope on every deployment. */
export function mediaDownloadUrl(href: string, base: string): string {
  const file = mediaFileUrl(href);
  if (file?.kind !== "media") throw new Error("Invalid attachment URL.");
  const route = new URL("media-download", base);
  route.searchParams.set("file", new URL(file.url.pathname, file.url).href);
  return route.href;
}

export function mediaRequestUrl(
  request: Pick<Request, "url" | "method" | "mode" | "destination">,
  base: string,
): URL | null {
  if (request.method !== "GET") return null;
  const url = new URL(request.url);
  const route = new URL("media-download", base);
  if (url.origin === route.origin && url.pathname === route.pathname) {
    // A relay must not let an embedded image or frame bypass external consent.
    if (request.mode !== "navigate" || request.destination !== "document")
      return null;
    const file = mediaFileUrl(url.searchParams.get("file") ?? "");
    if (file?.kind !== "media") return null;
    file.url.search = "?download=true";
    return file.url;
  }
  return mediaFileUrl(url.href)?.url ?? null;
}

export function isFileLink(
  value: unknown,
  base: string,
): value is { url: string } {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    !("url" in value) ||
    typeof value.url !== "string"
  )
    return false;
  try {
    const url = new URL(value.url);
    return (
      (url.protocol === "https:" ||
        (url.protocol === "http:" && new URL(base).protocol === "http:")) &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}
