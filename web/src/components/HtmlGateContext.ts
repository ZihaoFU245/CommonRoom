import { createContext } from "preact";

export type HtmlGateMode = "text" | "render" | null;
export interface HtmlGateChoice {
  messageKey: string;
  text: string;
  mode: Exclude<HtmlGateMode, null>;
}
export interface HtmlGateState {
  choices: ReadonlyMap<string, HtmlGateChoice>;
  change: (
    key: string,
    messageKey: string,
    text: string,
    mode: HtmlGateMode,
  ) => void;
}
export const HtmlGateContext = createContext<HtmlGateState | null>(null);
