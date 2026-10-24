import { createContext } from "preact";
import { useContext } from "preact/hooks";
import {
  nameColorOf,
  type NameColor,
} from "../features/preferences/nameColors.ts";

export const NameColorContext = createContext<Record<string, string>>({});

export function useNameColor(name: string): NameColor | undefined {
  return nameColorOf(useContext(NameColorContext), name);
}
