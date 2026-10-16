import { BotApiError, type BotApi } from "./api.ts";
import type { TaskRecord } from "./store.ts";
import type { Message } from "./wire.ts";

/** Revalidate nearby inputs in one bounded page; sparse ranges overlap four reads. */
export async function locateInputs(
  api: BotApi,
  conversation: string,
  tasks: TaskRecord[],
  signal: AbortSignal,
): Promise<{ messages: Map<string, Message>; requests: number }> {
  const groups: TaskRecord[][] = [];
  for (const task of [...tasks].sort((a, b) => a.sequence - b.sequence)) {
    const group = groups.at(-1);
    if (group && task.sequence - group[0].sequence < 100) group.push(task);
    else groups.push([task]);
  }
  const messages = new Map<string, Message>();
  let requests = 0;
  for (let offset = 0; offset < groups.length; offset += 4) {
    signal.throwIfAborted();
    await Promise.all(
      groups.slice(offset, offset + 4).map(async (group) => {
        const wanted = new Map(
          group.map((task) => [task.input, task.sequence]),
        );
        const end = group[group.length - 1].sequence;
        for (let attempt = 0; ; attempt++) {
          let start = group[0].sequence;
          let revision: number | undefined;
          const found = new Map<string, Message>();
          try {
            for (;;) {
              signal.throwIfAborted();
              requests++;
              const page = await api.history(
                {
                  conversation_id: conversation,
                  ...(group.length === 1
                    ? { around: start, limit: 1 }
                    : { start, end, limit: 100 }),
                  ...(revision === undefined
                    ? {}
                    : { content_revision: revision }),
                },
                signal,
              );
              signal.throwIfAborted();
              revision = page.content_revision;
              for (const message of page.messages)
                if (wanted.get(message.id) === message.sequence)
                  found.set(message.id, message);
              const last = page.messages.at(-1)?.sequence;
              if (
                group.length === 1 ||
                !page.has_after ||
                last === undefined ||
                last >= end
              )
                break;
              if (last < start)
                throw new Error("Input validation history did not advance");
              start = last + 1;
            }
          } catch (error) {
            // A byte-limited page can race an edit. Re-read the whole group once
            // without retaining any rows from the superseded content revision.
            if (
              attempt === 0 &&
              revision !== undefined &&
              error instanceof BotApiError &&
              error.status === 409
            )
              continue;
            throw error;
          }
          for (const [id, message] of found) messages.set(id, message);
          break;
        }
      }),
    );
  }
  return { messages, requests };
}
