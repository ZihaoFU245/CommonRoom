import { useEffect, useRef } from "preact/hooks";

export function DeleteConfirmation({
  kind,
  onCancel,
  onConfirm,
}: {
  kind: "message" | "resource";
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  return (
    <dialog
      ref={dialog}
      class="delete-confirmation"
      aria-labelledby="delete-title"
      aria-describedby="delete-description"
      onCancel={onCancel}
    >
      <h2 id="delete-title">
        {kind === "resource" ? "Confirm removal?" : "Delete message?"}
      </h2>
      <p id="delete-description">This action can’t be undone.</p>
      <div class="delete-confirmation-actions">
        <button type="button" autoFocus onClick={onCancel}>
          Cancel
        </button>
        <button type="button" class="confirm-delete" onClick={onConfirm}>
          {kind === "resource" ? "Remove" : "Delete message"}
        </button>
      </div>
    </dialog>
  );
}
