import type { Snapshot, Acknowledgement, Fonts } from "../../api/protocol.ts";
import type { Aliases } from "../../features/conversation/aliases.ts";
import type { Theme, Seeds } from "../../features/preferences/settings.ts";

export interface SettingsProps {
  state: Snapshot;
  pending: boolean;
  onAccountCommand: (text: string) => Promise<Acknowledgement>;
  aliases: Aliases;
  onSaveAlias: (name: string, target: string) => void;
  onRemoveAlias: (name: string) => void;
  fonts: Fonts;
  theme: Theme;
  seeds: Seeds;
  markdown: boolean;
  onMarkdownChange: (enabled: boolean) => void;
  latex: boolean;
  onLatexChange: (enabled: boolean) => void;
  allowHtml: boolean;
  onAllowHtmlChange: (enabled: boolean) => void;
  onChange: (value: Fonts) => void;
  onThemeChange: (value: Theme) => void;
  onSeedsChange: (value: Seeds) => void;
  onClose: () => void;
}
