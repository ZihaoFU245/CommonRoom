/** Test content without stripping joiners, variation selectors or other text. */
export function hasMessageContent(text: string): boolean {
  return /[^\p{White_Space}\p{Cc}\p{Default_Ignorable_Code_Point}\u2800\ufffc]/u.test(
    text,
  );
}
