import { createContext } from "preact";
import { useContext } from "preact/hooks";

export const DisplayNamesContext = createContext<Record<string, string>>({});

export function DisplayName({ name }: { name: string }) {
  const names = useContext(DisplayNamesContext);
  return (
    <span title={name}>{Object.hasOwn(names, name) ? names[name] : name}</span>
  );
}
