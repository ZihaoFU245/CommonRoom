import type {
  Acknowledgement,
  ClientFrame,
  PendingCommand,
} from "../../api/protocol.ts";

type Completion = (data: Acknowledgement, request: PendingCommand) => void;
interface Submission {
  command: Omit<PendingCommand, "id">;
  frame: Omit<ClientFrame, "id">;
  complete: Completion;
}

/** One in-flight operation per socket; failed operations are never replayed. */
export function createRequestLifecycle(
  transmit: (frame: ClientFrame) => void,
  pendingChanged: (pending: boolean) => void,
) {
  let serial = 0;
  let active: { request: PendingCommand; complete: Completion } | null = null;
  function finish(data: Acknowledgement) {
    if (!active || data.id !== active.request.id) return;
    const current = active;
    active = null;
    pendingChanged(false);
    current.complete(data, current.request);
  }
  return {
    get pending() {
      return active !== null;
    },
    submit({ command, frame, complete }: Submission): boolean {
      if (active) return false;
      const id = ++serial;
      active = { request: { ...command, id }, complete };
      pendingChanged(true);
      try {
        transmit({ ...frame, id });
      } catch (error: unknown) {
        finish({
          kind: "error",
          id,
          text:
            error instanceof Error
              ? error.message
              : "Command could not be sent.",
        });
      }
      return true;
    },
    receive: finish,
    disconnect: () => {
      if (active)
        finish({
          kind: "error",
          id: active.request.id,
          text: "Connection lost before confirmation. Check history before sending again.",
        });
    },
  };
}
