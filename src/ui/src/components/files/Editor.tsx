// CodeMirror 6 editor and merge view for the file explorer. The editor is
// loaded lazily (React.lazy) and each language loads on first use, so the
// console's main bundle stays small; everything is bundled locally for
// air-gapped installs.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { StreamLanguage, type StreamParser, bracketMatching, defaultHighlightStyle, foldGutter, foldKeymap, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { MergeView } from "@codemirror/merge";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { useEffect, useRef } from "react";

import type { Language } from "./lang";

export { LANGUAGES, type Language, detectLanguage } from "./lang";

const legacy = (p: Promise<StreamParser<unknown>>) => p.then((m) => StreamLanguage.define(m));

/** Loads a language's support on first use (one chunk per language family). */
export async function loadLanguage(l: Language): Promise<Extension> {
  switch (l) {
    case "json":
      return (await import("@codemirror/lang-json")).json();
    case "yaml":
      return (await import("@codemirror/lang-yaml")).yaml();
    case "xml":
      return (await import("@codemirror/lang-xml")).xml();
    case "html":
    case "vue":
      return (await import("@codemirror/lang-html")).html();
    case "markdown":
      return (await import("@codemirror/lang-markdown")).markdown();
    case "css":
      return (await import("@codemirror/lang-css")).css();
    case "scss":
      return legacy(import("@codemirror/legacy-modes/mode/css").then((m) => m.sCSS));
    case "less":
      return legacy(import("@codemirror/legacy-modes/mode/css").then((m) => m.less));
    case "javascript":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
    case "typescript":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true, typescript: true });
    case "php":
      return (await import("@codemirror/lang-php")).php();
    case "python":
      return (await import("@codemirror/lang-python")).python();
    case "sql":
      return (await import("@codemirror/lang-sql")).sql();
    case "ruby":
      return legacy(import("@codemirror/legacy-modes/mode/ruby").then((m) => m.ruby));
    case "perl":
      return legacy(import("@codemirror/legacy-modes/mode/perl").then((m) => m.perl));
    case "lua":
      return legacy(import("@codemirror/legacy-modes/mode/lua").then((m) => m.lua));
    case "r":
      return legacy(import("@codemirror/legacy-modes/mode/r").then((m) => m.r));
    case "go":
      return legacy(import("@codemirror/legacy-modes/mode/go").then((m) => m.go));
    case "rust":
      return legacy(import("@codemirror/legacy-modes/mode/rust").then((m) => m.rust));
    case "swift":
      return legacy(import("@codemirror/legacy-modes/mode/swift").then((m) => m.swift));
    case "java":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.java));
    case "kotlin":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.kotlin));
    case "scala":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.scala));
    case "c":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.c));
    case "cpp":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.cpp));
    case "csharp":
      return legacy(import("@codemirror/legacy-modes/mode/clike").then((m) => m.csharp));
    case "shell":
    case "makefile":
      return legacy(import("@codemirror/legacy-modes/mode/shell").then((m) => m.shell));
    case "powershell":
      return legacy(import("@codemirror/legacy-modes/mode/powershell").then((m) => m.powerShell));
    case "toml":
      return legacy(import("@codemirror/legacy-modes/mode/toml").then((m) => m.toml));
    case "ini":
    case "env":
    case "properties":
    case "systemd":
    case "batch":
    case "terraform":
    case "apache":
      return legacy(import("@codemirror/legacy-modes/mode/properties").then((m) => m.properties));
    case "nginx":
    case "caddyfile":
      return legacy(import("@codemirror/legacy-modes/mode/nginx").then((m) => m.nginx));
    case "dockerfile":
      return legacy(import("@codemirror/legacy-modes/mode/dockerfile").then((m) => m.dockerFile));
    case "cmake":
      return legacy(import("@codemirror/legacy-modes/mode/cmake").then((m) => m.cmake));
    case "protobuf":
      return legacy(import("@codemirror/legacy-modes/mode/protobuf").then((m) => m.protobuf));
    case "diff":
      return legacy(import("@codemirror/legacy-modes/mode/diff").then((m) => m.diff));
    case "graphql":
    case "log":
    case "plain":
      return [];
  }
}

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "12.5px", backgroundColor: "var(--color-panel)", color: "var(--color-fg)" },
  ".cm-scroller": { fontFamily: "var(--font-mono, ui-monospace, monospace)" },
  ".cm-gutters": { backgroundColor: "var(--color-panel-2)", color: "var(--color-faint)", border: "none" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "color-mix(in oklab, var(--color-accent) 7%, transparent)" },
  "&.cm-focused .cm-cursor": { borderLeftColor: "var(--color-accent)" },
  ".cm-panels": { backgroundColor: "var(--color-panel-2)", color: "var(--color-fg)" },
});

function base(readOnly: boolean): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    foldGutter(),
    history(),
    drawSelection(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    bracketMatching(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, ...foldKeymap, indentWithTab]),
    EditorState.readOnly.of(readOnly),
    EditorView.editable.of(!readOnly),
    theme,
  ];
}

/** Applies `lang` to a view's compartment once it has loaded. */
function useLanguage(view: React.RefObject<EditorView | null>, comp: Compartment, lang: Language, key: unknown) {
  useEffect(() => {
    let live = true;
    void loadLanguage(lang).then((ext) => {
      if (live) view.current?.dispatch({ effects: comp.reconfigure(ext) });
    });
    return () => {
      live = false;
    };
  }, [view, comp, lang, key]);
}

/** A CodeMirror editor; `value` is read once per `docKey` (reload/open). */
export function CodeEditor({
  value,
  docKey,
  lang,
  readOnly,
  wrap,
  onChange,
  onSave,
}: {
  value: string;
  docKey: string;
  lang: Language;
  readOnly: boolean;
  wrap: boolean;
  onChange: (v: string) => void;
  onSave?: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const langC = useRef(new Compartment());
  const wrapC = useRef(new Compartment());
  const cb = useRef({ onChange, onSave });
  cb.current = { onChange, onSave };

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          base(readOnly),
          langC.current.of([]),
          wrapC.current.of(wrap ? EditorView.lineWrapping : []),
          keymap.of([{ key: "Mod-s", preventDefault: true, run: () => (cb.current.onSave?.(), true) }]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) cb.current.onChange(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // A new document (or read-only switch) builds a new editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey, readOnly]);

  useLanguage(view, langC.current, lang, `${docKey}:${readOnly}`);
  useEffect(() => {
    view.current?.dispatch({ effects: wrapC.current.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }, [wrap]);

  return <div ref={host} className="h-full min-h-0 overflow-hidden" />;
}

/**
 * Side-by-side comparison: the file as it is now on the node (left,
 * read-only) and your edit (right, editable); `onChange` tracks the merge.
 */
export function MergeEditor({ current, mine, lang, onChange }: { current: string; mine: string; lang: Language; onChange: (v: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const cb = useRef(onChange);
  cb.current = onChange;
  useEffect(() => {
    if (!host.current) return;
    let mv: MergeView | null = null;
    let live = true;
    void loadLanguage(lang).then((ext) => {
      if (!live || !host.current) return;
      mv = new MergeView({
        parent: host.current,
        a: { doc: current, extensions: [base(true), ext] },
        b: {
          doc: mine,
          extensions: [base(false), ext, EditorView.updateListener.of((u) => u.docChanged && cb.current(u.state.doc.toString()))],
        },
        gutter: true,
        highlightChanges: true,
      });
    });
    return () => {
      live = false;
      mv?.destroy();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, lang]);
  return <div ref={host} className="h-full min-h-0 overflow-auto" />;
}

export default CodeEditor;
