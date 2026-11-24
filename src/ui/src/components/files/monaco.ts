// Monaco (the VS Code editor) set up once: the editor features, syntax
// highlighting for every basic language, and JSON with its language worker.
// The heavy TypeScript, HTML and CSS language services are left out. Loaded
// lazily with the editor chunk, and bundled locally for air-gapped installs.
import "monaco-editor/features/register.all";
import "monaco-editor/languages/definitions/register.all";
import "monaco-editor/languages/features/json/register";

import * as monaco from "monaco-editor/editor/editor.api";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/languages/features/json/json.worker?worker";

import type { Language } from "./lang";

self.MonacoEnvironment = {
  getWorker: (_id: string, label: string) => (label === "json" ? new JsonWorker() : new EditorWorker()),
};

export { monaco };

/** Our language names mapped to Monaco's; unknown ones show as plain text. */
const IDS: Partial<Record<Language, string>> = {
  ini: "ini",
  env: "ini",
  properties: "ini",
  toml: "ini",
  systemd: "ini",
  apache: "ini",
  json: "json",
  yaml: "yaml",
  xml: "xml",
  html: "html",
  vue: "html",
  markdown: "markdown",
  css: "css",
  scss: "scss",
  less: "less",
  javascript: "javascript",
  typescript: "typescript",
  php: "php",
  python: "python",
  ruby: "ruby",
  perl: "perl",
  lua: "lua",
  r: "r",
  go: "go",
  rust: "rust",
  java: "java",
  kotlin: "kotlin",
  scala: "scala",
  c: "cpp",
  cpp: "cpp",
  csharp: "csharp",
  swift: "swift",
  shell: "shell",
  makefile: "shell",
  powershell: "powershell",
  batch: "bat",
  sql: "sql",
  nginx: "shell",
  caddyfile: "shell",
  dockerfile: "dockerfile",
  terraform: "hcl",
  graphql: "graphql",
  protobuf: "protobuf",
};

export const monacoLanguage = (l: Language) => IDS[l] ?? "plaintext";

/** Light+ or Dark+, following the console theme (html[data-theme]). */
const pick = () => (document.documentElement.dataset.theme === "dark" ? "vs-dark" : "vs");

let watching = false;
/** Applies the console theme now and whenever it changes. */
export function followTheme() {
  monaco.editor.setTheme(pick());
  if (watching) return;
  watching = true;
  new MutationObserver(() => monaco.editor.setTheme(pick())).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
}

/** Options every editor shares. */
export const BASE: monaco.editor.IStandaloneEditorConstructionOptions = {
  automaticLayout: true,
  fontFamily: '"JetBrains Mono", "Cascadia Code", Consolas, ui-monospace, monospace',
  fontSize: 13,
  lineHeight: 19,
  tabSize: 2,
  scrollBeyondLastLine: false,
  renderLineHighlight: "all",
  smoothScrolling: true,
  fixedOverflowWidgets: true,
  stickyScroll: { enabled: false },
  bracketPairColorization: { enabled: true },
};
