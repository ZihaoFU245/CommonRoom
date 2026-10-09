import { useEffect, useRef } from "preact/hooks";

export function DeleteConfirmation({
  onCancel,
  onConfirm,
}: {
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
      <h2 id="delete-title">Delete message?</h2>
      <p id="delete-description">
        This deletes the message for everyone in the conversation and removes
        its reply previews. This cannot be undone.
      </p>
      <div class="delete-confirmation-actions">
        <button type="button" autoFocus onClick={onCancel}>
          Cancel
        </button>
        <button type="button" class="confirm-delete" onClick={onConfirm}>
          Delete message
        </button>
      </div>
    </dialog>
  );
}
