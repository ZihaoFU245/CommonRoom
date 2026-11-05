/** Test content without stripping joiners, variation selectors or other text. */
export function hasMessageContent(text: string): boolean {
  return /[^\p{White_Space}\p{Cc}\p{Default_Ignorable_Code_Point}\u2800\ufffc]/u.test(
    text,
  );
}

const singleEmojiPattern = (() => {
  try {
    return new RegExp("^\\p{RGI_Emoji}$", "v");
  } catch {
    // Older engines can still render the message at its regular size.
    return undefined;
  }
})();

export function singleEmoji(text: string): string | undefined {
  const trimmed = text.trim();
  return singleEmojiPattern?.test(trimmed) ? trimmed : undefined;
}
