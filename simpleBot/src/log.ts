// Minimal shared logging contract.

export type LogLevel = "debug" | "info" | "warn" | "error";

export type Logger = (level: LogLevel, message: string) => void;

export const LEVELS: Record<string, number> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
  silent: 99,
};
