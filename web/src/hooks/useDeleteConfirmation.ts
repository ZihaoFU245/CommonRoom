import { useRef, useState } from "preact/hooks";

export function useDeleteConfirmation() {
  const [deleteConfirmation, setDeleteConfirmation] = useState<
    "message" | "resource" | null
  >(null);
  const deleteAction = useRef<(() => void) | null>(null);
  function requestDelete(
    action: () => void,
    kind: "message" | "resource" = "message",
  ) {
    deleteAction.current = action;
    setDeleteConfirmation(kind);
  }
  function cancelDelete() {
    deleteAction.current = null;
    setDeleteConfirmation(null);
  }
  function confirmDelete() {
    const action = deleteAction.current;
    cancelDelete();
    action?.();
  }
  return { deleteConfirmation, requestDelete, cancelDelete, confirmDelete };
}
