import type { SettingsProps } from "./types.ts";
import { defaultSeeds, parseHex } from "../../features/preferences/palette.ts";
import {
  defaultFonts,
  defaultTheme,
  type Theme,
} from "../../features/preferences/settings.ts";

const themes: { value: Theme; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
  { value: "custom", label: "Custom" },
];

function SeedField({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const parsed = parseHex(value);
  return (
    <div class="palette-seed">
      <label for={id}>{label}</label>
      <input
        class="palette-swatch"
        type="color"
        aria-label={`${label} color picker`}
        value={parsed ?? defaultSeeds.base}
        onInput={(event) => onChange(event.currentTarget.value)}
      />
      <input
        id={id}
        class="palette-hex"
        type="text"
        spellcheck={false}
        autoComplete="off"
        maxLength={7}
        value={value}
        aria-invalid={parsed === null}
        onInput={(event) => onChange(event.currentTarget.value)}
      />
    </div>
  );
}

export function AppearanceSettings({
  fonts,
  theme,
  seeds,
  onChange,
  onThemeChange,
  onSeedsChange,
}: Pick<
  SettingsProps,
  "fonts" | "theme" | "seeds" | "onChange" | "onThemeChange" | "onSeedsChange"
>) {
  return (
    <>
      <fieldset class="theme-setting">
        <legend>Theme</legend>
        <div class="theme-choices">
          {themes.map((choice) => (
            <label class="theme-choice" key={choice.value}>
              <input
                type="radio"
                name="theme"
                value={choice.value}
                checked={theme === choice.value}
                onChange={() => onThemeChange(choice.value)}
              />
              <span>{choice.label}</span>
            </label>
          ))}
        </div>
      </fieldset>
      {theme === "custom" && (
        <div class="palette-seeds">
          <SeedField
            id="palette-base"
            label="Surface"
            value={seeds.base}
            onChange={(base) => onSeedsChange({ ...seeds, base })}
          />
          <SeedField
            id="palette-accent"
            label="Accent"
            value={seeds.accent}
            onChange={(accent) => onSeedsChange({ ...seeds, accent })}
          />
          <hr class="palette-divider" />
        </div>
      )}
      <div class="font-setting">
        <label for="chat-font-size">Chat font size</label>
        <output for="chat-font-size">{fonts.chat}px</output>
        <input
          id="chat-font-size"
          type="range"
          min="14"
          max="24"
          step="1"
          value={fonts.chat}
          onInput={(event) =>
            onChange({
              ...fonts,
              chat: Number(event.currentTarget.value),
            })
          }
        />
      </div>
      <div class="font-setting">
        <label for="ui-font-size">UI font size</label>
        <output for="ui-font-size">{fonts.ui}px</output>
        <input
          id="ui-font-size"
          type="range"
          min="12"
          max="18"
          step="1"
          value={fonts.ui}
          onInput={(event) =>
            onChange({
              ...fonts,
              ui: Number(event.currentTarget.value),
            })
          }
        />
      </div>
      <button
        class="settings-reset"
        onClick={() => {
          onChange({ ...defaultFonts });
          onThemeChange(defaultTheme);
          onSeedsChange({ ...defaultSeeds });
        }}
      >
        Reset defaults
      </button>
    </>
  );
}
