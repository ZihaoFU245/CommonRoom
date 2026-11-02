import { serverBaseUrl } from "../api/server.ts";
import type { AttachmentContext } from "./Attachment.tsx";
import { ExternalImage } from "./ExternalImage.tsx";
import { MediaDownload } from "./MediaDownload.tsx";
import { ManagedAttachment } from "./ManagedAttachment.tsx";
import {
  attachmentLink,
  managedMediaLink,
  stickerImage,
} from "../api/media.ts";
import { StickerImage } from "./StickerImage.tsx";
import { createElement } from "preact";
import type { ComponentChildren } from "preact";
import { lazy, memo, Suspense } from "preact/compat";
import type { MarkdownNode } from "../features/conversation/markdown.ts";
import { mentionedText } from "../features/conversation/interactions.ts";
import { Mention } from "./Mention.tsx";
const Attachment = lazy(() =>
  import("./Attachment.tsx").then((module) => ({ default: module.Attachment })),
);
const Math = lazy(() =>
  import("./Math.tsx")
    .then((module) => ({ default: module.Math }))
    .catch(() => ({
      default: ({ source }: { source: string }) => <>{source}</>,
    })),
);
export const MarkdownContent = memo(function MarkdownContent({
  nodes,
  mentions,
  renderHtml,
  attachmentContext,
}: {
  nodes: MarkdownNode[];
  mentions: string[];
  renderHtml?: (source: string, index: number) => ComponentChildren;
  attachmentContext?: AttachmentContext;
}) {
  function render(nodes: MarkdownNode[], code = false): ComponentChildren {
    return nodes.map((node, index) => {
      if (typeof node === "string")
        return code
          ? node
          : mentionedText(node, mentions).map((part, i) =>
              part.mention ? <Mention key={i} text={part.text} /> : part.text,
            );
      if (node.html)
        return (
          renderHtml?.(node.html.source, node.html.index) ?? node.html.source
        );
      if (node.math)
        return (
          <Suspense key={index} fallback={node.math.source}>
            <Math source={node.math.source} display={node.math.display} />
          </Suspense>
        );
      const attachment =
        node.href && node.title
          ? attachmentLink(node.href, node.title, serverBaseUrl())
          : null;
      const managed = managedMediaLink(
        node.image?.src ?? node.href ?? "",
        serverBaseUrl(),
      );
      function wrap(content: ComponentChildren) {
        return managed ? (
          <ManagedAttachment key={`${index}:${managed.id}`} id={managed.id}>
            {content}
          </ManagedAttachment>
        ) : (
          content
        );
      }
      if (attachment) {
        const fallback = (
          <MediaDownload
            id={attachment.id}
            href={new URL(node.href ?? attachment.path, serverBaseUrl()).href}
          >
            {render(node.children)}
          </MediaDownload>
        );
        return wrap(
          <Suspense key={attachment.id + attachment.kind} fallback={fallback}>
            <Attachment
              id={attachment.id}
              href={new URL(node.href ?? attachment.path, serverBaseUrl()).href}
              kind={attachment.kind}
              {...(attachmentContext ? { context: attachmentContext } : {})}
            >
              {render(node.children)}
            </Attachment>
          </Suspense>,
        );
      }
      const sticker = node.image
        ? stickerImage(node.image.src, node.image.title, document.baseURI)
        : null;
      if (sticker && node.image)
        return wrap(
          <StickerImage
            key={`${index}:${sticker.id}`}
            id={sticker.id}
            src={node.image.src}
          />,
        );
      if (node.image)
        return wrap(
          <ExternalImage
            key={`${index}:${node.image.src}`}
            src={node.image.src}
            alt={node.image.alt}
          />,
        );
      if (managed && node.href && node.tag === "a")
        return wrap(
          <MediaDownload
            id={managed.id}
            href={new URL(node.href, serverBaseUrl()).href}
          >
            {render(node.children)}
          </MediaDownload>,
        );
      return wrap(
        createElement(
          node.tag === "p" &&
            node.children.some(
              (child) =>
                typeof child !== "string" && child.title?.startsWith("media:"),
            )
            ? "div"
            : node.tag,
          {
            key: index,
            ...(node.href
              ? {
                  href: managed
                    ? new URL(node.href, serverBaseUrl()).href
                    : node.href,
                  target: "_blank",
                  rel: "noopener noreferrer",
                }
              : {}),
            ...(node.start ? { start: node.start } : {}),
          },
          render(node.children, code || node.tag === "code"),
        ),
      );
    });
  }
  return (
    <div dir="auto" class="message-markdown">
      {render(nodes)}
    </div>
  );
});
