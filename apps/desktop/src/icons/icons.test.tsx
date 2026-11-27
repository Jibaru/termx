import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import styles from "../styles.css?raw";
import type { SessionKind } from "../lib/ipc";
import * as glyphs from "./glyphs";
import { KindIcon } from "./KindIcon";
import { StatusIcon } from "./StatusIcon";
import { type IconStatus, STATUS_TEXT } from "./statusText";

const STATUSES = Object.keys(STATUS_TEXT) as IconStatus[];
const KINDS: SessionKind[] = ["claude", "shell", "background"];

function shapeOf(element: Element): string {
  return element.getAttribute("data-shape") ?? "";
}

describe("the status icons", () => {
  it("give every state a shape of its own and a name", () => {
    render(
      <div>
        {STATUSES.map((status) => (
          <StatusIcon key={status} status={status} />
        ))}
      </div>,
    );

    const icons = screen.getAllByRole("img");
    expect(icons).toHaveLength(STATUSES.length);
    const shapes = icons.map(shapeOf);
    expect(new Set(shapes).size).toBe(STATUSES.length);
    for (const status of STATUSES) {
      const icon = screen.getByRole("img", { name: STATUS_TEXT[status].label });
      expect(shapeOf(icon)).toBe(`status-${status}`);
      // The tooltip explains the state.
      expect(icon.querySelector("title")?.textContent).toBe(STATUS_TEXT[status].description);
    }
  });

  it("draw the same shape at every size, in the colour of the state", () => {
    const { container } = render(
      <div>
        <StatusIcon status="failed" size={12} />
        <StatusIcon status="failed" size={20} />
      </div>,
    );
    const [small, large] = [...container.querySelectorAll("svg")];

    expect(small?.getAttribute("width")).toBe("12");
    expect(large?.getAttribute("width")).toBe("20");
    expect(small?.innerHTML.replace(/<title>.*<\/title>/, "")).toBe(
      large?.innerHTML.replace(/<title>.*<\/title>/, ""),
    );
    expect(small?.getAttribute("data-tone")).toBe("danger");
  });

  it("make the two states that wait for the user the loudest, and the only ones in that colour", () => {
    const loud = STATUSES.filter((status) => STATUS_TEXT[status].tone === "attention");
    expect(loud.sort()).toEqual(["waitingForInput", "waitingForPermission"]);
  });

  it("can say something more specific, or step aside for the words next to them", () => {
    render(
      <div>
        <StatusIcon status="failed" title="Claude Code is not installed." />
        <StatusIcon status="running" decorative />
      </div>,
    );

    expect(screen.getByRole("img", { name: "Failed" }).querySelector("title")).toHaveTextContent(
      "Claude Code is not installed.",
    );
    expect(screen.getAllByRole("img")).toHaveLength(1);
  });

  it("move slowly, and not at all for people who ask for less motion", () => {
    expect(styles).toMatch(/@media \(prefers-reduced-motion: reduce\)[^}]*\.icon-spin/);
    expect(styles).toMatch(/\.icon-breathe \{\s*animation: icon-breathe 2\.4s/);
  });
});

describe("the kind icons", () => {
  it("give every kind a shape of its own and a name", () => {
    render(
      <div>
        {KINDS.map((kind) => (
          <KindIcon key={kind} kind={kind} decorative={false} />
        ))}
      </div>,
    );

    const icons = screen.getAllByRole("img");
    expect(new Set(icons.map(shapeOf)).size).toBe(KINDS.length);
    for (const name of ["Claude Code", "Shell", "Background command"]) {
      expect(screen.getByRole("img", { name })).toBeInTheDocument();
    }
  });

  it("draw a kind this version does not know as a shell", () => {
    const { container } = render(<KindIcon kind={"robot" as SessionKind} />);

    expect(shapeOf(container.querySelector("svg") ?? document.body)).toBe("kind-shell");
  });
});

describe("the glyphs of the interface", () => {
  it("each have a shape of their own, and are decorative unless named", () => {
    const icons = Object.entries(glyphs);
    const { container } = render(
      <div>
        {icons.map(([name, Icon]) => (
          <Icon key={name} />
        ))}
      </div>,
    );
    const svgs = [...container.querySelectorAll("svg")];

    expect(new Set(svgs.map(shapeOf)).size).toBe(icons.length);
    expect(svgs.every((svg) => svg.getAttribute("aria-hidden") === "true")).toBe(true);

    render(<glyphs.WarningIcon label="Warning" />);
    expect(screen.getByRole("img", { name: "Warning" })).toBeInTheDocument();
  });
});
