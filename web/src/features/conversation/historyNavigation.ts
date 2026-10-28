/** Show the bottom shortcut after three screens, retaining it until arrival. */
export function bottomShortcut(
  distance: number,
  height: number,
  hasAfter: boolean,
  alreadyVisible: boolean,
): boolean {
  if (!hasAfter && distance <= 32) return false;
  return distance >= height * 3 || alreadyVisible;
}

/** A deleted or expired destination resolves to the next retained row. */
export function retainedJumpSequence(
  sequences: number[],
  requested: number,
): number | undefined {
  return (
    sequences.find((sequence) => sequence >= requested) ?? sequences.at(-1)
  );
}

/** One stable destination per unread batch, independent of read acknowledgements. */
export class UnreadJumpTargets {
  private targets = new Map<string, { sequence: number; used: boolean }>();

  target(
    identity: string,
    first: number | null | undefined,
    count: number,
  ): number | undefined {
    if (!first || count <= 0) {
      this.targets.delete(identity);
      return undefined;
    }
    let entry = this.targets.get(identity);
    if (!entry) {
      entry = { sequence: first, used: false };
      this.targets.set(identity, entry);
      if (this.targets.size > 32) {
        const oldest = this.targets.keys().next().value;
        if (oldest !== undefined) this.targets.delete(oldest);
      }
    }
    return entry.used ? undefined : entry.sequence;
  }

  consume(identity: string): number | undefined {
    const entry = this.targets.get(identity);
    if (!entry || entry.used) return undefined;
    entry.used = true;
    return entry.sequence;
  }
}
