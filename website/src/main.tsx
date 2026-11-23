import { StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import "./styles/site.css";
import { App, PAGES, normalize } from "./App";

const root = document.getElementById("root")!;
const path = normalize(window.location.pathname);
// nginx serves the pre-rendered 404 page for unknown paths.
const known = path in PAGES ? path : "/404";
const app = (
  <StrictMode>
    <App path={known} />
  </StrictMode>
);

// Pre-rendered pages hydrate; `npm run dev` renders from scratch.
if (root.hasChildNodes()) hydrateRoot(root, app);
else createRoot(root).render(app);
