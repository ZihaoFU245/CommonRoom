import type { RefObject } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "../api/protocol.ts";
import type { ValueRef } from "./refs.ts";
import { debugAccess } from "../features/conversation/permissions.ts";
import { commandText, privatePeers } from "../features/conversation/console.ts";

export function useCommandNavigation({
  permissions,
  state,
  currentState,
  desiredRoom,
  setSelected,
  clearView,
  logout,
}: {
  permissions: string[];
  state: Snapshot;
  currentState: ValueRef<Snapshot>;
  desiredRoom: RefObject<string | null>;
  setSelected: (view: string) => void;
  clearView: () => void;
  logout: () => void;
}) {
  const [debug, setDebug] = useState(false);
  const [sudoDebug, setSudoDebug] = useState(false);
  const canDebug = debugAccess(
    permissions,
    sudoDebug,
    state.permissions,
    state.commands,
  );
  useEffect(() => {
    if (!canDebug) {
      setDebug(false);
      setSudoDebug(false);
    }
  }, [canDebug]);
  function acknowledge(text: string) {
    const [command, argument, name] = commandText(text).split(/\s+/u);
    const target =
      command === "/room"
        ? name?.replace(/^#/u, "")
        : command === "/new"
          ? argument === "room"
            ? name
            : undefined
          : argument;
    if (command === "/clear") clearView();
    if (command === "/logout") logout();
    if (command === "/console") {
      desiredRoom.current = null;
      setSelected("@command");
    }
    if (
      (["/new", "/join"].includes(command || "") ||
        (command === "/room" && ["new", "join"].includes(argument || ""))) &&
      target
    ) {
      desiredRoom.current = target;
      if (currentState.current.rooms.some((room) => room.name === target)) {
        setSelected(target);
        desiredRoom.current = null;
      }
    }
    if (command === "/debug") {
      setDebug(target === "on");
      setSudoDebug(target === "on" && text.trim().split(/\s+/u)[0] === "/sudo");
    }
    if (command === "/tell" && target) {
      desiredRoom.current = `@direct:${target}`;
      if (privatePeers(currentState.current).includes(target)) {
        setSelected(desiredRoom.current);
        desiredRoom.current = null;
      }
    }
  }
  return { debug: debug && canDebug, acknowledge };
}
