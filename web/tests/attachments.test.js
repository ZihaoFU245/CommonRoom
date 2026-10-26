import assert from "node:assert/strict";
import test from "node:test";
import {
  canAttachFile,
  clipboardAttachment,
} from "../src/features/conversation/attachments.ts";

const idle = {
  enabled: true,
  consoleView: false,
  canWrite: true,
  pending: false,
  uploading: false,
  attached: false,
  draft: "",
};

function image(name, type = "image/png") {
  return new File([new Uint8Array([137, 80, 78, 71])], name, { type });
}

function fileItem(file) {
  return { kind: "file", type: file.type, getAsFile: () => file };
}

function stringItem(text) {
  return { kind: "string", type: "text/plain", getAsFile: () => null };
}

function clipboard({ items = [], files = [] } = {}) {
  return { items, files };
}

test("attaching stays available exactly while the composer can take a file", () => {
  assert.equal(canAttachFile(idle), true);
  assert.equal(canAttachFile({ ...idle, draft: "hello" }), true);
  assert.equal(canAttachFile({ ...idle, enabled: false }), false);
  assert.equal(canAttachFile({ ...idle, consoleView: true }), false);
  assert.equal(canAttachFile({ ...idle, canWrite: false }), false);
  assert.equal(canAttachFile({ ...idle, pending: true }), false);
  assert.equal(canAttachFile({ ...idle, uploading: true }), false);
  assert.equal(canAttachFile({ ...idle, attached: true }), false);
  assert.equal(canAttachFile({ ...idle, draft: "/help" }), false);
  assert.equal(canAttachFile({ ...idle, draft: "  /help" }), false);
});

test("an empty clipboard or a text-only clipboard is not an attachment", () => {
  assert.equal(clipboardAttachment(null), null);
  assert.equal(clipboardAttachment(clipboard()), null);
  assert.equal(
    clipboardAttachment(clipboard({ items: [stringItem("hello")] })),
    null,
  );
});

test("non-image clipboard files keep the ordinary paste behavior", () => {
  const document = new File(["%PDF"], "report.pdf", {
    type: "application/pdf",
  });
  assert.equal(
    clipboardAttachment(clipboard({ items: [fileItem(document)] })),
    null,
  );
});

test("the first image in the clipboard becomes the attachment", () => {
  const png = image("image.png");
  const jpeg = image("photo.jpg", "image/jpeg");
  assert.equal(
    clipboardAttachment(clipboard({ items: [fileItem(png), fileItem(jpeg)] })),
    png,
  );
  assert.equal(clipboardAttachment(clipboard({ files: [jpeg] })), jpeg);
});

test("unnamed screenshots receive the extension of their image type", () => {
  const png = clipboardAttachment(clipboard({ items: [fileItem(image(""))] }));
  assert.equal(png?.name, "pasted-image.png");
  assert.equal(png?.type, "image/png");
  assert.equal(
    clipboardAttachment(
      clipboard({ items: [fileItem(image("", "image/jpeg"))] }),
    )?.name,
    "pasted-image.jpeg",
  );
  assert.equal(
    clipboardAttachment(
      clipboard({ items: [fileItem(image("", "image/svg+xml"))] }),
    )?.name,
    "pasted-image.svg",
  );
});
