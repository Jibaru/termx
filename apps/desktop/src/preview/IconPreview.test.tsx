import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { STATUS_TEXT } from "../icons/statusText";
import { ICON_BACKGROUNDS, ICON_ROLES } from "../theme/palette";
import { IconPreview } from "./IconPreview";

describe("the icon preview", () => {
  it("shows every state at three sizes on every background, and the palette", () => {
    render(<IconPreview />);

    const states = Object.keys(STATUS_TEXT).length;
    expect(screen.getAllByRole("img")).toHaveLength(states * 3 * ICON_BACKGROUNDS.length);
    expect(screen.getAllByRole("row")).toHaveLength(ICON_ROLES.length + 1);
  });
});
