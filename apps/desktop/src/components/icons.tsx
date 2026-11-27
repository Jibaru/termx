import type { SessionKind } from "../lib/ipc";

/** Small line icons, drawn in the current text colour. Decorative only. */

interface IconProps {
  className?: string;
}

function Svg({ className, children }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      viewBox="0 0 16 16"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** An eight-pointed spark, for Claude Code sessions. */
export function ClaudeIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2" />
    </Svg>
  );
}

/** A prompt, for shells. */
export function ShellIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M2.5 4l4 4-4 4M8 12.5h5.5" />
    </Svg>
  );
}

/** A gear-like cycle, for background commands. */
export function BackgroundIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v2.6h-2.6" />
    </Svg>
  );
}

export function KindIcon({ kind, className }: IconProps & { kind: SessionKind }) {
  const name = className ? `kind-icon ${className}` : "kind-icon";
  switch (kind) {
    case "claude":
      return <ClaudeIcon className={`${name} kind-claude`} />;
    case "background":
      return <BackgroundIcon className={`${name} kind-background`} />;
    default:
      return <ShellIcon className={`${name} kind-shell`} />;
  }
}

export function BranchIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="4.5" cy="12.5" r="1.5" />
      <circle cx="11.5" cy="5.5" r="1.5" />
      <path d="M4.5 5v6M11.5 7c0 3-7 2-7 4" />
    </Svg>
  );
}

export function ChevronIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M6 4l4 4-4 4" />
    </Svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M8 3v10M3 8h10" />
    </Svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M4 4l8 8M12 4l-8 8" />
    </Svg>
  );
}

export function SplitRightIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M8 3v10" />
    </Svg>
  );
}

export function SplitDownIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M2 8h12" />
    </Svg>
  );
}

export function SidebarIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M6 3v10" />
    </Svg>
  );
}

export function MoreIcon(props: IconProps) {
  return (
    <Svg {...props}>
      <path d="M3.5 8h.01M8 8h.01M12.5 8h.01" strokeWidth="2.5" />
    </Svg>
  );
}
