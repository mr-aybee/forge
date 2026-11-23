import { useEffect, useState } from "react";
import { Monitor, Moon, Sun } from "lucide-react";

type Theme = "auto" | "light" | "dark";
const ORDER: Theme[] = ["auto", "light", "dark"];
// Same key as Starlight, so the website and /docs/ share the choice.
const KEY = "starlight-theme";

function apply(theme: Theme) {
  const root = document.documentElement;
  if (theme === "auto") {
    root.dataset.theme = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } else {
    root.dataset.theme = theme;
  }
}

export function ThemeToggle() {
  // Starts as "auto" so server and first client render agree; the stored
  // choice is read after hydration.
  const [theme, setTheme] = useState<Theme>("auto");

  useEffect(() => {
    try {
      const stored = localStorage.getItem(KEY);
      if (stored === "light" || stored === "dark") setTheme(stored);
    } catch {
      // Storage can be blocked; fall back to the system preference.
    }
  }, []);

  useEffect(() => {
    apply(theme);
    if (theme !== "auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => apply("auto");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [theme]);

  const next = () => {
    const t = ORDER[(ORDER.indexOf(theme) + 1) % ORDER.length];
    setTheme(t);
    try {
      localStorage.setItem(KEY, t === "auto" ? "" : t);
    } catch {
      // Not persisted; the choice still applies to this page.
    }
  };

  const Ico = theme === "dark" ? Moon : theme === "light" ? Sun : Monitor;
  return (
    <button
      type="button"
      onClick={next}
      className="inline-flex size-9 items-center justify-center rounded-full text-muted hover:bg-sunken hover:text-fg"
      aria-label={`Theme: ${theme}. Change theme`}
      title={`Theme: ${theme}`}
    >
      <Ico className="size-4" />
    </button>
  );
}
