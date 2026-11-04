import stickerIcon from "../assets/emoji-sticker.svg";

export function PickerIcon({ name }: { name: string }) {
  if (name === "sticker")
    return (
      <span
        class="picker-icon picker-sticker-icon"
        aria-hidden="true"
        style={{ maskImage: `url("${stickerIcon}")` }}
      />
    );
  const paths: Record<string, string> = {
    // Refresh double-arrow geometry supplied by the user (SVG Repo).
    refresh:
      "M21.1679 8C19.6247 4.46819 16.1006 2 11.9999 2C6.81459 2 2.55104 5.94668 2.04932 11M17 8H21.4C21.7314 8 22 7.73137 22 7.4V3M2.88146 16C4.42458 19.5318 7.94874 22 12.0494 22C17.2347 22 21.4983 18.0533 22 13M7.04932 16H2.64932C2.31795 16 2.04932 16.2686 2.04932 16.6V21",
    recent: "M12 8v4l3 2M21 12a9 9 0 1 1-3-6.7M21 3v5h-5",
    emoji: "M8 14c2 3 6 3 8 0M8 9h.01M16 9h.01",
    nature:
      "M12 21c-8-4-10-12-8-17 6 0 10 3 12 7M12 21c8-4 10-10 8-15-6 0-10 5-8 15Zm0 0L7 10",
    food: "M6 3v7m4-7v7M3 3v6a5 5 0 0 0 5 5v7M18 21V3c-4 3-4 10 0 10",
    travel: "m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8",
    activity:
      "M12 3 14.8 8.7 21 9.6 16.5 14 17.6 20.2 12 17.3 6.4 20.2 7.5 14 3 9.6 9.2 8.7 12 3Z",
    object: "M9 18h6m-6 3h6M8 13a6 6 0 1 1 8 0l-1 3H9l-1-3Z",
    symbol: "M12 21 3.5 12.5a5.5 5.5 0 0 1 8.5-7 5.5 5.5 0 0 1 8.5 7L12 21Z",
    flag: "M5 21V3c5-3 9 3 14 0v11c-5 3-9-3-14 0",
    search: "m16 16 5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0Z",
    close: "m6 6 12 12M18 6 6 18",
    manage: "M4 7h16M4 17h16M9 4v6m6 4v6",
    upload: "M12 16V3m-5 5 5-5 5 5M4 15v5h16v-5",
    plus: "M12 5v14M5 12h14",
    owned: "m8 12 3 3 5-6M12 3l8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z",
  };
  return (
    <svg
      class="picker-icon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      style={name === "refresh" ? { strokeWidth: 1.5 } : undefined}
    >
      {name === "emoji" && <circle cx="12" cy="12" r="9" />}
      <path d={paths[name] ?? paths.emoji} />
    </svg>
  );
}
