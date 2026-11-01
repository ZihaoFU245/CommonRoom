import { UserCardTrigger } from "./UserCardTrigger.tsx";
import { DisplayNamesContext } from "./DisplayNamesContext.ts";
import { useContext } from "preact/hooks";

export function DisplayName({
  name,
  interactive = false,
}: {
  name: string;
  interactive?: boolean;
}) {
  const names = useContext(DisplayNamesContext);
  const label = Object.hasOwn(names, name) ? (names[name] ?? name) : name;
  if (interactive)
    return (
      <UserCardTrigger name={name} label={label}>
        {label}
      </UserCardTrigger>
    );
  return (
    <span title={name}>{Object.hasOwn(names, name) ? names[name] : name}</span>
  );
}
