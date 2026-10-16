export function preview(text: string, limit: number): string {
  const single = text.replace(/\s+/g, " ").trim();
  const characters = Array.from(single);
  return characters.length <= limit
    ? single
    : `${characters.slice(0, limit).join("")}…`;
}
