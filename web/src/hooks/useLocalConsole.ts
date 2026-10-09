import type { RefObject } from "preact";
import { useRef, useState } from "preact/hooks";
import type { Snapshot, LocalOutput, Ledger } from "../api/protocol.ts";
import { ingest, redactCommand } from "../features/conversation/console.ts";
export function useLocalConsole(
  initial: Snapshot,
  selected: string,
  selectedRef: RefObject<string>,
  atBottom: RefObject<boolean>,
) {
  const [output, setOutput] = useState<LocalOutput[]>([]);
  const [cleared, setCleared] = useState<Record<string, number>>({});
  const [initialLedger] = useState(() => ingest(initial));
  const ledger = useRef<Ledger>(initialLedger);
  function append(
    command: string,
    result: string | null = null,
    error = false,
    view = selected,
  ) {
    command = redactCommand(command);
    if (view === selectedRef.current) atBottom.current = true;
    const order = ++ledger.current.sequence;
    const entry: LocalOutput = {
      kind: "command",
      key: `local-${order}`,
      order,
      command,
      result,
      error,
      room: view,
    };
    setOutput((previous) => [...previous, entry].slice(-200));
    return entry.key;
  }
  function complete(key: string, result: string, error = false) {
    setOutput((previous) =>
      previous.map((entry) =>
        entry.key === key ? { ...entry, result, error } : entry,
      ),
    );
  }

  function clearView() {
    setOutput((previous) =>
      previous.filter((entry) => entry.room !== selected),
    );
    setCleared((previous) => ({
      ...previous,
      [selected]: ledger.current.sequence,
    }));
  }
  return { output, cleared, setCleared, ledger, append, complete, clearView };
}
