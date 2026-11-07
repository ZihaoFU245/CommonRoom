import expiredAttachmentIcon from "../assets/attachment-expired.svg?no-inline";

export function AttachmentPlaceholder({
  filename,
  label,
}: {
  filename: string;
  label: string;
}) {
  return (
    <span class="attachment-expired" role="status">
      <img
        class="attachment-expired-icon"
        src={expiredAttachmentIcon}
        width="32"
        height="32"
        alt=""
        aria-hidden="true"
        draggable={false}
      />
      <span class="attachment-expired-copy">
        <span class="attachment-expired-name" dir="auto">
          {filename}
        </span>
        <span class="attachment-expired-label">{label}</span>
      </span>
    </span>
  );
}
