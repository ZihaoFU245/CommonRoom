import type { Snapshot } from "../../api/protocol.ts";

function sameStrings(a: readonly string[], b: readonly string[]): boolean {
  return (
    a === b ||
    (a.length === b.length && a.every((value, index) => value === b[index]))
  );
}

/** Compare only fields that change media visibility, without serializing snapshots. */
export function sameMediaAuthority(a: Snapshot, b: Snapshot): boolean {
  if (a === b) return true;
  if (
    a.sync?.instance !== b.sync?.instance ||
    a.account_access.id !== b.account_access.id ||
    a.policy_revision !== b.policy_revision ||
    !sameStrings(a.permissions, b.permissions) ||
    !sameStrings(a.users, b.users) ||
    a.rooms.length !== b.rooms.length
  )
    return false;
  if (
    !a.rooms.every((room, index) => {
      const other = b.rooms[index];
      return (
        other !== undefined &&
        room.id === other.id &&
        sameStrings(room.permissions, other.permissions) &&
        sameStrings(room.members, other.members)
      );
    })
  )
    return false;
  const keys = Object.keys(a.private_access);
  return (
    keys.length === Object.keys(b.private_access).length &&
    keys.every((key) => {
      const left = a.private_access[key];
      const right = b.private_access[key];
      return (
        left !== undefined &&
        right !== undefined &&
        left.id === right.id &&
        sameStrings(left.permissions, right.permissions)
      );
    })
  );
}
