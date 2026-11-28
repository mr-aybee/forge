// An nginx configuration language for Monaco (it ships none): directives,
// blocks, variables, strings, numbers with units, regex locations and comments.
import type { languages } from "monaco-editor/editor/editor.api";

/** Block directives, coloured as keywords. */
const BLOCKS = [
  "events", "http", "server", "location", "upstream", "map", "geo", "if", "limit_except",
  "types", "split_clients", "stream", "mail", "match",
];

export const nginxConfiguration: languages.LanguageConfiguration = {
  comments: { lineComment: "#" },
  brackets: [["{", "}"], ["(", ")"]],
  autoClosingPairs: [
    { open: "{", close: "}" },
    { open: "(", close: ")" },
    { open: '"', close: '"', notIn: ["string"] },
    { open: "'", close: "'", notIn: ["string"] },
  ],
  surroundingPairs: [
    { open: "{", close: "}" },
    { open: '"', close: '"' },
    { open: "'", close: "'" },
  ],
  folding: { markers: { start: /\{\s*(#.*)?$/, end: /^\s*\}/ } },
};

export const nginxTokens: languages.IMonarchLanguage = {
  defaultToken: "",
  tokenPostfix: ".nginx",
  blocks: BLOCKS,
  tokenizer: {
    // A statement starts with its directive name.
    root: [
      { include: "@space" },
      [/[{}]/, "delimiter.bracket"],
      [/;/, "delimiter"],
      [/[a-zA-Z_][\w.-]*/, { cases: { "@blocks": { token: "keyword", next: "@args" }, "@default": { token: "type.identifier", next: "@args" } } }],
      [/\$\{?\w+\}?/, { token: "variable", next: "@args" }],
      [/[^\s;{}]+/, { token: "", next: "@args" }],
    ],
    // Its arguments run up to `;` or `{`.
    args: [
      { include: "@space" },
      [/;/, { token: "delimiter", next: "@pop" }],
      [/\{/, { token: "delimiter.bracket", next: "@pop" }],
      [/\}/, { token: "delimiter.bracket", next: "@pop" }],
      [/\$\{?\w+\}?/, "variable"],
      [/"/, { token: "string.quote", next: "@dstring" }],
      [/'/, { token: "string.quote", next: "@sstring" }],
      [/(~\*?|\^~|=)(?=\s)/, "operator"],
      [/\b(on|off)\b(?=[\s;])/, "keyword"],
      [/\b\d+(\.\d+)*(:\d+)?(\/\d+)?([kKmMgG]|ms|[smhdwMy])?\b/, "number"],
      [/[^\s;{}"'$]+/, ""],
    ],
    dstring: [
      [/\$\{?\w+\}?/, "variable"],
      [/\\./, "string.escape"],
      [/"/, { token: "string.quote", next: "@pop" }],
      [/[^"\\$]+/, "string"],
      [/./, "string"],
    ],
    sstring: [
      [/\$\{?\w+\}?/, "variable"],
      [/\\./, "string.escape"],
      [/'/, { token: "string.quote", next: "@pop" }],
      [/[^'\\$]+/, "string"],
      [/./, "string"],
    ],
    space: [
      [/\s+/, "white"],
      [/#.*$/, "comment"],
    ],
  },
};
