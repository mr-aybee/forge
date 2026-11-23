import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type Theme = "light" | "dark";
/** Colours of log and output consoles (not SSH terminals, which have their own scheme). */
export type ConsoleTheme = "dark" | "light";

interface Prefs {
  theme: Theme;
  toggleTheme: () => void;
  /** Optional environment filter applied to infrastructure views. */
  environment?: string;
  setEnvironment: (id?: string) => void;
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
  consoleTheme: ConsoleTheme;
  setConsoleTheme: (t: ConsoleTheme) => void;
}

const Ctx = createContext<Prefs | null>(null);

function read<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage unavailable: preference just isn't remembered */
  }
}

/** Per-browser UI preferences. Light theme is the default. */
export function PrefsProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<Theme>(() => read<Theme>("forge.theme", "light"));
  const [environment, setEnvironment] = useState<string | undefined>(undefined);
  const [sidebarCollapsed, setCollapsed] = useState(() => read("forge.sidebar", false));
  // Log consoles follow the app theme on every page load; the console toggle
  // overrides it for this visit only and is not saved. Switching the app theme
  // drops the override so both match again.
  const [consoleOverride, setConsoleTheme] = useState<ConsoleTheme>();
  const consoleTheme: ConsoleTheme = consoleOverride ?? theme;

  useEffect(() => {
    document.documentElement.dataset.console = consoleTheme;
  }, [consoleTheme]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    write("forge.theme", theme);
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((t) => (t === "light" ? "dark" : "light"));
    setConsoleTheme(undefined);
  }, []);
  const toggleSidebar = useCallback(
    () =>
      setCollapsed((c) => {
        write("forge.sidebar", !c);
        return !c;
      }),
    [],
  );

  const value = useMemo(
    () => ({ theme, toggleTheme, environment, setEnvironment, sidebarCollapsed, toggleSidebar, consoleTheme, setConsoleTheme }),
    [theme, toggleTheme, environment, sidebarCollapsed, toggleSidebar, consoleTheme],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function usePrefs(): Prefs {
  const c = useContext(Ctx);
  if (!c) throw new Error("usePrefs outside PrefsProvider");
  return c;
}
