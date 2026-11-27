import "../styles.css";

import { createRoot } from "react-dom/client";

import { IconPreview } from "./IconPreview";

// The entry of icon-preview.html, for development only.
const root = document.getElementById("root");
if (root) createRoot(root).render(<IconPreview />);
