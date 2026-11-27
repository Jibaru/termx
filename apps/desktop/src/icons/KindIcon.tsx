import type { SessionKind } from "../lib/ipc";
import { Glyph, type IconSize } from "./Glyph";

const KIND_LABELS: Record<SessionKind, string> = {
  claude: "Claude Code",
  shell: "Shell",
  background: "Background command",
};

interface KindIconProps {
  kind: SessionKind;
  size?: IconSize;
  /** Leave it out of the accessibility tree, when the words are next to it. */
  decorative?: boolean;
}

/**
 * What a session runs, each in a colour of its own:
 *
 * - Claude Code: an eight-pointed spark with a filled heart.
 * - Shell: a prompt, `>_`, in a window.
 * - Background command: a cycle, the arrows of something that keeps going.
 */
export function KindIcon({ kind, size = 16, decorative = true }: KindIconProps) {
  const known: SessionKind = kind === "claude" || kind === "background" ? kind : "shell";
  return (
    <Glyph
      shape={`kind-${known}`}
      className={`kind-icon kind-${known}`}
      tone={`kind-${known}`}
      size={size}
      label={decorative ? undefined : KIND_LABELS[known]}
    >
      {known === "claude" ? (
        <>
          <path d="M8 1.75v3.5M8 10.75v3.5M1.75 8h3.5M10.75 8h3.5M3.6 3.6l2.45 2.45M9.95 9.95l2.45 2.45M12.4 3.6 9.95 6.05M6.05 9.95 3.6 12.4" />
          <circle cx="8" cy="8" r="1.4" fill="currentColor" stroke="none" />
        </>
      ) : known === "background" ? (
        <>
          <path d="M13.25 7A5.25 5.25 0 0 0 3.6 4.6" />
          <path d="M3.25 1.75v3h3" />
          <path d="M2.75 9a5.25 5.25 0 0 0 9.65 2.4" />
          <path d="M12.75 14.25v-3h-3" />
        </>
      ) : (
        <>
          <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2" />
          <path d="m4.75 6.25 2 1.75-2 1.75M8.25 10.25h3" />
        </>
      )}
    </Glyph>
  );
}
