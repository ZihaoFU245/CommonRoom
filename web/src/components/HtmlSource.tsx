export function HtmlSource({ text }: { text: string }) {
  return (
    <pre class="html-gate-source" tabIndex={0}>
      <code>{text}</code>
    </pre>
  );
}
