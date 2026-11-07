import autoAnimate from "@formkit/auto-animate";
import { useEffect, useRef, useState } from "preact/hooks";
import { savedServerUrl, saveServerUrl } from "../api/server.ts";
import { errorMessage } from "../api/protocol.ts";
import { controlMotion } from "../features/preferences/motion.ts";

export function LoginServerSettings({
  disabled,
  onSave,
}: {
  disabled: boolean;
  onSave: (url: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  const [error, setError] = useState("");
  const container = useRef<HTMLDivElement>(null);
  const motion = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null);

  function close() {
    setOpen(false);
    trigger.current?.focus();
  }

  useEffect(() => {
    const element = motion.current;
    if (!element) return;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    const animation = autoAnimate(element, (child, action) => {
      const collapsed = { opacity: 0, transform: "scale(0.15)" };
      const expanded = { opacity: 1, transform: "scale(1)" };
      const effect = new KeyframeEffect(
        child,
        action === "remove"
          ? [expanded, collapsed]
          : action === "add"
            ? [collapsed, expanded]
            : [expanded],
        {
          ...controlMotion,
          duration:
            reduced.matches || action === "remain" ? 0 : controlMotion.duration,
        },
      );
      if (action === "remove") {
        child.setAttribute("inert", "");
        // Preserve bottom anchoring and the gear origin during removal, including
        // when a new card opens before this one's exit animation finishes.
        return [effect, { styleReset: false }];
      }
      return effect;
    });
    const sync = () => {
      if (reduced.matches) {
        animation.disable();
        for (const active of element.getAnimations({ subtree: true }))
          active.finish();
      } else animation.enable();
    };
    sync();
    reduced.addEventListener("change", sync);
    return () => {
      reduced.removeEventListener("change", sync);
      animation.destroy?.();
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    input.current?.focus();
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !container.current?.contains(event.target)
      )
        close();
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  return (
    <div
      class="login-server-settings"
      ref={container}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.stopPropagation();
          close();
        }
      }}
    >
      <div ref={motion} class="login-server-motion">
        {open && (
          <form
            class="login-server-card"
            id="login-server-card"
            role="dialog"
            aria-labelledby="login-server-title"
            onSubmit={(event) => {
              event.preventDefault();
              try {
                const url = saveServerUrl(value.trim());
                close();
                onSave(url);
              } catch (error) {
                setError(errorMessage(error));
              }
            }}
          >
            <strong id="login-server-title">Server settings</strong>
            <label for="login-server-url">Server URL</label>
            <input
              ref={input}
              id="login-server-url"
              type="text"
              inputMode="url"
              autoComplete="url"
              autoCapitalize="none"
              spellcheck={false}
              placeholder="chat.example.com/server"
              value={value}
              aria-invalid={!!error}
              aria-describedby={error ? "login-server-error" : undefined}
              onInput={(event) => {
                setValue(event.currentTarget.value);
                setError("");
              }}
            />
            {error && (
              <div id="login-server-error" class="form-error" role="alert">
                {error}
              </div>
            )}
            <div class="login-server-actions">
              <button type="button" onClick={close}>
                Cancel
              </button>
              <button class="primary" type="submit">
                Save
              </button>
            </div>
          </form>
        )}
      </div>
      <button
        ref={trigger}
        type="button"
        class="login-server-trigger"
        aria-label="Server settings"
        title="Server settings"
        aria-expanded={open}
        aria-controls={open ? "login-server-card" : undefined}
        aria-haspopup="dialog"
        disabled={disabled}
        onClick={() => {
          if (open) close();
          else {
            setValue(savedServerUrl());
            setError("");
            setOpen(true);
          }
        }}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m9.5 3-.6 2.3-1.7 1-2.3-.6-2.5 4.3 1.7 1.7v2L2.4 15l2.5 4.3 2.3-.6 1.7 1 .6 2.3h5l.6-2.3 1.7-1 2.3.6 2.5-4.3-1.7-1.7v-2l1.7-1.7-2.5-4.3-2.3.6-1.7-1L14.5 3z" />
          <circle cx="12" cy="12.5" r="3" />
        </svg>
      </button>
    </div>
  );
}
