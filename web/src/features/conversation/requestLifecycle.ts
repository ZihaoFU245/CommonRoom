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
  parallel?: boolean;
}

/** Commands stay exclusive; up to 16 ordinary messages may await acknowledgement. */
export function createRequestLifecycle(
  transmit: (frame: ClientFrame) => void,
  pendingChanged: (pending: boolean) => void,
) {
  let serial = 0;
  const active = new Map<
    number,
    { request: PendingCommand; complete: Completion; parallel: boolean }
  >();
  const idle = new Set<() => void>();
  const exclusive = () => [...active.values()].some((item) => !item.parallel);
  const available = (parallel: boolean) =>
    parallel ? !exclusive() && active.size < 16 : active.size === 0;
  function finish(data: Acknowledgement) {
    const current = active.get(data.id);
    if (!current) return;
    active.delete(data.id);
    pendingChanged(exclusive());
    current.complete(data, current.request);
    if (!active.size) {
      for (const callback of [...idle]) {
        if (active.size) break;
        idle.delete(callback);
        callback();
      }
    }
  }
  return {
    whenIdle(this: void, callback: () => void) {
      if (!active.size) callback();
      else idle.add(callback);
      return () => {
        idle.delete(callback);
      };
    },
    get pending() {
      return active.size > 0;
    },
    canSubmitMessage() {
      return available(true);
    },
    submit({
      command,
      frame,
      complete,
      parallel = false,
    }: Submission): boolean {
      if (!available(parallel)) return false;
      const id = ++serial;
      active.set(id, { request: { ...command, id }, complete, parallel });
      pendingChanged(exclusive());
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
      for (const id of [...active.keys()])
        finish({
          kind: "error",
          id,
          text: "Connection lost before confirmation. Check history before sending again.",
        });
    },
  };
}
