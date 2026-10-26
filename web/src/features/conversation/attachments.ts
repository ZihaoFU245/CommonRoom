/** Everything that decides whether the composer may start an upload. */
export interface AttachmentAvailability {
  /** The server exposes media configuration. */
  enabled: boolean;
  /** Console output never carries attachments. */
  consoleView: boolean;
  /** The conversation allows writing. */
  canWrite: boolean;
  /** A message is being sent. */
  pending: boolean;
  /** An upload is allocating, transferring or verifying. */
  uploading: boolean;
  /** A completed attachment is already waiting in the composer. */
  attached: boolean;
  draft: string;
}

/** The attach control and pasted images share one availability rule. */
export function canAttachFile({
  enabled,
  consoleView,
  canWrite,
  pending,
  uploading,
  attached,
  draft,
}: AttachmentAvailability): boolean {
  return (
    enabled &&
    !consoleView &&
    canWrite &&
    !pending &&
    !uploading &&
    !attached &&
    !draft.trim().startsWith("/")
  );
}

/** Screenshots often arrive unnamed, and allocation rejects an empty name. */
export function pastedImageName(file: File): string {
  if (file.name) return file.name;
  const [family = ""] = file.type
    .slice("image/".length)
    .toLowerCase()
    .split("+");
  const extension = /^[a-z0-9]+$/u.test(family) ? family : "png";
  return `pasted-image.${extension}`;
}

function firstImage(data: DataTransfer | null): File | null {
  const items = data?.items;
  if (items) {
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      if (item?.kind !== "file" || !item.type.startsWith("image/")) continue;
      const file = item.getAsFile();
      if (file) return file;
    }
  }
  const files = data?.files;
  if (files) {
    for (let index = 0; index < files.length; index += 1) {
      const file = files[index];
      if (file?.type.startsWith("image/")) return file;
    }
  }
  return null;
}

/**
 * The file a paste would attach, or null when the clipboard carries no image.
 * Text, rich-text and copied files keep the browser's ordinary paste behavior.
 */
export function clipboardAttachment(data: DataTransfer | null): File | null {
  const file = firstImage(data);
  if (!file || file.name) return file;
  return new File([file], pastedImageName(file), { type: file.type });
}
