import type { IconSize } from "../icons/Glyph";
import * as glyphs from "../icons/glyphs";
import { KindIcon } from "../icons/KindIcon";
import { StatusIcon } from "../icons/StatusIcon";
import { type IconStatus, STATUS_TEXT } from "../icons/statusText";
import {
  contrast,
  DEFAULT_PALETTE,
  ICON_BACKGROUNDS,
  ICON_ROLES,
  type PaletteRole,
} from "../theme/palette";

/**
 * Every icon at every size on every background the interface draws them
 * on, and the palette with its contrast ratios: a way to look at them
 * without starting termx. Development only (see icon-preview.html).
 */

const SIZES: IconSize[] = [12, 16, 20];
const STATUSES = Object.keys(STATUS_TEXT) as IconStatus[];
const KINDS = ["claude", "shell", "background"] as const;
const GLYPHS = Object.entries(glyphs);

const cell = { display: "flex", alignItems: "center", gap: 8, padding: "4px 0" } as const;

function Section({ background }: { background: PaletteRole }) {
  return (
    <section
      style={{
        backgroundColor: DEFAULT_PALETTE[background],
        border: `1px solid ${DEFAULT_PALETTE.border}`,
        borderRadius: 6,
        padding: "8px 12px",
        minWidth: 0,
      }}
    >
      <h2 style={{ margin: "0 0 6px", fontSize: 12, color: DEFAULT_PALETTE.textMuted }}>
        on --color-{background} {DEFAULT_PALETTE[background]}
      </h2>
      {STATUSES.map((status) => (
        <div key={status} style={cell}>
          {SIZES.map((size) => (
            <StatusIcon key={size} status={status} size={size} />
          ))}
          <span style={{ fontSize: 12 }}>{STATUS_TEXT[status].label}</span>
        </div>
      ))}
      <div style={cell}>
        {KINDS.map((kind) =>
          SIZES.map((size) => <KindIcon key={`${kind}${String(size)}`} kind={kind} size={size} />),
        )}
      </div>
      <div style={{ ...cell, flexWrap: "wrap", color: DEFAULT_PALETTE.textMuted }}>
        {GLYPHS.map(([name, Glyph]) => (
          <Glyph key={name} size={16} />
        ))}
      </div>
    </section>
  );
}

function Swatches() {
  return (
    <table style={{ borderCollapse: "collapse", fontSize: 12, fontFamily: "var(--font-mono)" }}>
      <thead>
        <tr>
          <th style={{ textAlign: "left", padding: "2px 8px" }}>role</th>
          <th style={{ textAlign: "left", padding: "2px 8px" }}>colour</th>
          {ICON_BACKGROUNDS.map((background) => (
            <th key={background} style={{ padding: "2px 8px" }}>
              vs {background}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {ICON_ROLES.map((role) => (
          <tr key={role}>
            <td style={{ padding: "2px 8px" }}>{role}</td>
            <td style={{ padding: "2px 8px" }}>
              <span
                style={{
                  display: "inline-block",
                  width: 12,
                  height: 12,
                  marginRight: 6,
                  verticalAlign: "middle",
                  backgroundColor: DEFAULT_PALETTE[role],
                }}
              />
              {DEFAULT_PALETTE[role]}
            </td>
            {ICON_BACKGROUNDS.map((background) => {
              const ratio = contrast(DEFAULT_PALETTE[role], DEFAULT_PALETTE[background]);
              return (
                <td
                  key={background}
                  style={{
                    padding: "2px 8px",
                    textAlign: "right",
                    color: ratio >= 3 ? DEFAULT_PALETTE.text : DEFAULT_PALETTE.danger,
                  }}
                >
                  {ratio.toFixed(2)}:1
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** The whole preview page. */
export function IconPreview() {
  return (
    <main style={{ padding: 16, overflow: "auto", height: "100%" }}>
      <h1 style={{ margin: "0 0 12px", fontSize: 16 }}>termx icons and palette</h1>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
          gap: 12,
          marginBottom: 16,
        }}
      >
        {ICON_BACKGROUNDS.map((background) => (
          <Section key={background} background={background} />
        ))}
      </div>
      <Swatches />
    </main>
  );
}
