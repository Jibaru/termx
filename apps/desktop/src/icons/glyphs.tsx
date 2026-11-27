import { Glyph, type GlyphProps } from "./Glyph";

/**
 * The small glyphs of the interface, in the language of `Glyph`: line
 * drawings on the 16-unit grid, decorative unless given a label.
 */

export function PlusIcon(props: GlyphProps) {
  return (
    <Glyph shape="plus" {...props}>
      <path d="M8 3v10M3 8h10" />
    </Glyph>
  );
}

export function CloseIcon(props: GlyphProps) {
  return (
    <Glyph shape="close" {...props}>
      <path d="m4 4 8 8m0-8-8 8" />
    </Glyph>
  );
}

export function SplitRightIcon(props: GlyphProps) {
  return (
    <Glyph shape="split-right" {...props}>
      <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2" />
      <path d="M8 2.75v10.5" />
      <path d="M10.25 8h1.75" />
    </Glyph>
  );
}

export function SplitDownIcon(props: GlyphProps) {
  return (
    <Glyph shape="split-down" {...props}>
      <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2" />
      <path d="M1.75 8h12.5" />
      <path d="M8 10v1.5" />
    </Glyph>
  );
}

export function ResumeIcon(props: GlyphProps) {
  return (
    <Glyph shape="resume" {...props}>
      <path d="M4.75 3.25v9.5L12.5 8Z" fill="currentColor" />
    </Glyph>
  );
}

export function FolderIcon(props: GlyphProps) {
  return (
    <Glyph shape="folder" {...props}>
      <path d="M1.75 4.25a1.5 1.5 0 0 1 1.5-1.5h3l1.5 1.75h5a1.5 1.5 0 0 1 1.5 1.5v5.75a1.5 1.5 0 0 1-1.5 1.5h-9.5a1.5 1.5 0 0 1-1.5-1.5Z" />
    </Glyph>
  );
}

export function BranchIcon(props: GlyphProps) {
  return (
    <Glyph shape="branch" {...props}>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="4.5" cy="12.5" r="1.5" />
      <circle cx="11.5" cy="5" r="1.5" />
      <path d="M4.5 5v6M11.5 6.5c0 2.75-7 2-7 4.5" />
    </Glyph>
  );
}

/** A linked worktree: a second tree grown from the same branch. */
export function WorktreeIcon(props: GlyphProps) {
  return (
    <Glyph shape="worktree" {...props}>
      <rect x="1.75" y="1.75" width="7.5" height="7.5" rx="1.5" />
      <path d="M6.75 11.25v1.5a1.5 1.5 0 0 0 1.5 1.5h4.5a1.5 1.5 0 0 0 1.5-1.5v-4.5a1.5 1.5 0 0 0-1.5-1.5h-1.5" />
    </Glyph>
  );
}

export function WarningIcon(props: GlyphProps) {
  return (
    <Glyph shape="warning" {...props}>
      <path d="M8 1.75 14.5 13.5h-13Z" />
      <path d="M8 6v3.5" />
      <path d="M8 11.75h.01" strokeWidth="2" />
    </Glyph>
  );
}

export function InfoIcon(props: GlyphProps) {
  return (
    <Glyph shape="info" {...props}>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 7.25v4" />
      <path d="M8 4.75h.01" strokeWidth="2" />
    </Glyph>
  );
}

export function ChevronIcon(props: GlyphProps) {
  return (
    <Glyph shape="chevron" {...props}>
      <path d="m6 3.75 4.25 4.25L6 12.25" />
    </Glyph>
  );
}

export function SidebarIcon(props: GlyphProps) {
  return (
    <Glyph shape="sidebar" {...props}>
      <rect x="1.75" y="2.75" width="12.5" height="10.5" rx="2" />
      <path d="M6 2.75v10.5" />
    </Glyph>
  );
}

export function MoreIcon(props: GlyphProps) {
  return (
    <Glyph shape="more" {...props}>
      <path d="M3.5 8h.01M8 8h.01M12.5 8h.01" strokeWidth="2.25" />
    </Glyph>
  );
}
