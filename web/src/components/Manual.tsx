import { manualBlocks } from "../features/conversation/manual.ts";

function inlineCode(text: string) {
  return text
    .split(/(`[^`]+`)/u)
    .map((part, index) =>
      part.startsWith("`") && part.endsWith("`") && part.length > 2 ? (
        <code key={index}>{part.slice(1, -1)}</code>
      ) : (
        part
      ),
    );
}

export function Manual({ text }: { text: string }) {
  return (
    <div class="manual">
      {manualBlocks(text).map((block, index) => {
        switch (block.kind) {
          case "title":
            return <h2 key={index}>{inlineCode(block.text)}</h2>;
          case "heading":
            return <h3 key={index}>{inlineCode(block.text)}</h3>;
          case "subheading":
            return <h4 key={index}>{inlineCode(block.text)}</h4>;
          case "code":
            return (
              <pre key={index}>
                <code>{block.text}</code>
              </pre>
            );
          case "list":
            return (
              <ul
                key={index}
                class={
                  block.items.every((item) => /^`[^`]+`$/u.test(item))
                    ? "manual-code-list"
                    : undefined
                }
              >
                {block.items.map((item, itemIndex) => (
                  <li key={itemIndex}>{inlineCode(item)}</li>
                ))}
              </ul>
            );
          case "paragraph":
            return <p key={index}>{inlineCode(block.text)}</p>;
        }
      })}
    </div>
  );
}
