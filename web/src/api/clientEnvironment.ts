type ClientEnvironment = Pick<
  Navigator,
  "userAgent" | "platform" | "maxTouchPoints"
> & {
  userAgentData?: {
    brands: readonly { brand: string }[];
    mobile: boolean;
    platform: string;
  };
  standalone?: boolean;
};

// Three small axes, packed once per document. Zero on each axis means unknown.
export function clientCohort(
  browser: ClientEnvironment,
  display: "browser" | "standalone" | "unknown",
): number {
  const ua = browser.userAgent;
  const hints = browser.userAgentData;
  const ipad =
    /iPad/u.test(ua) ||
    (browser.platform === "MacIntel" &&
      browser.maxTouchPoints > 1 &&
      /AppleWebKit/u.test(ua));
  const ios = ipad || /iPhone|iPod/u.test(ua) || hints?.platform === "iOS";
  const blink =
    hints?.brands.some(({ brand }) => brand === "Chromium") ||
    /(?:Chrome|Chromium|Edg|OPR)\//u.test(ua);
  const engine = ios
    ? /AppleWebKit\//u.test(ua)
      ? 2
      : 0
    : blink
      ? 1
      : /Gecko\/\d/u.test(ua)
        ? 3
        : /AppleWebKit\//u.test(ua)
          ? 2
          : 0;
  const android = /Android/u.test(ua) || hints?.platform === "Android";
  const device =
    ipad || (android && hints?.mobile !== true && !/Mobile/u.test(ua))
      ? 3
      : ios || android || hints?.mobile || /Mobile/u.test(ua)
        ? 2
        : /Windows NT|Macintosh|X11|CrOS/u.test(ua) ||
            ["Windows", "macOS", "Linux", "Chrome OS"].includes(
              hints?.platform ?? "",
            )
          ? 1
          : 0;
  const mode =
    browser.standalone === true || display === "standalone"
      ? 2
      : display === "browser" || browser.standalone === false
        ? 1
        : 0;
  return (engine * 4 + device) * 3 + mode;
}
