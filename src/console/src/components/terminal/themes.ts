import type { ITheme } from "@xterm/xterm";

export interface TermTheme {
  id: string;
  label: string;
  theme: ITheme;
}

/** Terminal colour schemes. "Ocean" matches the console's palette. */
export const TERM_THEMES: TermTheme[] = [
  {
    id: "ocean",
    label: "Ocean",
    theme: {
      background: "#06121f",
      foreground: "#d7e6f5",
      cursor: "#4cc3dd",
      cursorAccent: "#06121f",
      selectionBackground: "#1f6fc466",
      black: "#0b1d30",
      red: "#f27474",
      green: "#5fd3a5",
      yellow: "#f2c86b",
      blue: "#4f9cf0",
      magenta: "#b48ef0",
      cyan: "#4cc3dd",
      white: "#c9d8e8",
      brightBlack: "#4d6a88",
      brightRed: "#ff9696",
      brightGreen: "#86e6c0",
      brightYellow: "#ffdb91",
      brightBlue: "#7db8ff",
      brightMagenta: "#cfb0ff",
      brightCyan: "#82e0f0",
      brightWhite: "#f2f7fc",
    },
  },
  {
    id: "frost",
    label: "Frost (light)",
    theme: {
      background: "#f4f8fc",
      foreground: "#14283d",
      cursor: "#1f6fc4",
      cursorAccent: "#f4f8fc",
      selectionBackground: "#4cc3dd55",
      black: "#14283d",
      red: "#c4352f",
      green: "#1d8a5c",
      yellow: "#a46b00",
      blue: "#1f6fc4",
      magenta: "#7a48c4",
      cyan: "#0e8aa3",
      white: "#c6d3e0",
      brightBlack: "#5a7088",
      brightRed: "#e0504a",
      brightGreen: "#27a86f",
      brightYellow: "#c78a10",
      brightBlue: "#3a8ae0",
      brightMagenta: "#9564e0",
      brightCyan: "#18a7c2",
      brightWhite: "#ffffff",
    },
  },
  {
    id: "classic",
    label: "Classic",
    theme: { background: "#000000", foreground: "#e5e5e5", cursor: "#e5e5e5", selectionBackground: "#ffffff44" },
  },
  {
    id: "midnight",
    label: "Midnight",
    theme: {
      background: "#0d1117",
      foreground: "#c9d1d9",
      cursor: "#58a6ff",
      selectionBackground: "#388bfd55",
      black: "#484f58",
      red: "#ff7b72",
      green: "#3fb950",
      yellow: "#d29922",
      blue: "#58a6ff",
      magenta: "#bc8cff",
      cyan: "#39c5cf",
      white: "#b1bac4",
      brightBlack: "#6e7681",
      brightRed: "#ffa198",
      brightGreen: "#56d364",
      brightYellow: "#e3b341",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#56d4dd",
      brightWhite: "#f0f6fc",
    },
  },
];

export const themeById = (id: string) => TERM_THEMES.find((t) => t.id === id) ?? TERM_THEMES[0]!;
