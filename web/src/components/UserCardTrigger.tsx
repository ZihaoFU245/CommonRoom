import { createContext, type ComponentChildren } from "preact";
import { useContext } from "preact/hooks";
import { DisplayNamesContext } from "./DisplayNamesContext.ts";

export const UserCardContext = createContext<
  (
    name: string,
    label: string,
    anchor: HTMLElement,
    origin: { x: number; y: number },
  ) => void
>(() => {});

export function UserCardTrigger({
  name,
  label,
  children,
  avatar = false,
}: {
  name: string;
  label?: string;
  children: ComponentChildren;
  avatar?: boolean;
}) {
  const open = useContext(UserCardContext);
  const names = useContext(DisplayNamesContext);
  const display =
    label ?? (Object.hasOwn(names, name) ? names[name] : undefined) ?? name;
  return (
    <button
      type="button"
      class={`user-card-trigger${avatar ? " avatar-trigger" : ""}`}
      aria-label={`View ${display}'s profile`}
      aria-haspopup="dialog"
      onClick={(event) => {
        event.stopPropagation();
        const anchor = event.currentTarget;
        const bounds = anchor.getBoundingClientRect();
        open(
          name,
          display,
          anchor,
          event.detail === 0
            ? {
                x: bounds.left + bounds.width / 2,
                y: bounds.top + bounds.height / 2,
              }
            : { x: event.clientX, y: event.clientY },
        );
      }}
    >
      {children}
    </button>
  );
}
