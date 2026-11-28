#!/usr/bin/env python3
"""Check or apply the blank-line style rules from CLAUDE.md to Rust sources.

Usage:
    python3 scripts/dev/style_check.py --check [paths...]   # exit 1 on violations
    python3 scripts/dev/style_check.py --fix [paths...]     # insert missing blank lines

With no paths it scans every .rs file under src/forge/. Run `cargo fmt --all` first:
the scan relies on rustfmt's layout (one statement start per line, `{` at the
end of a block header). It only ever inserts blank lines; rustfmt already
removes blank lines after `{`, before `}` and collapses runs of blank lines.

Rules applied inside fn, method, closure and async bodies (not match arms,
not macro bodies):
  1. a blank line before a comment that starts a new step;
  2. a blank line before and after if / match / for / while / loop / let-else
     statements and statements whose value is such a block (`let x = match`);
  3. a blank line before return / break / continue when statements precede
     them (a two-statement block such as `push(x); continue;` stays tight);
  4. a blank line before the tail expression of a fn body with two or more
     statements before it;
  7. a single-line `let` immediately checked by the next if / match stays
     adjacent to it.
"""

import argparse
import re
import sys
from pathlib import Path

CTL_KW = re.compile(r"(?<![\w.:])(if|match|while|for)(?!\w)")
ITEM_RE = re.compile(
    r"^(#\s*(\[.*?\]|\(\))\s*)*(pub(\([^)]*\))?\s+)?(unsafe\s+)?(struct|enum|union|trait|impl|mod|extern)\b", re.S
)
FN_RE = re.compile(r"(?<![\w.])fn\b")
JUMP_RE = re.compile(r"^(return|break|continue)\b")
LET_NAMES = re.compile(r"\b(?:mut\s+)?([a-z_]\w*)\b")

PROCESSED = {"fn", "block", "closure", "async"}


class Level:
    def __init__(self, kind, ctl, macro, line):
        self.kind = kind  # fn|block|closure|async|match|item|other|macro|paren
        self.ctl = ctl
        self.macro = macro
        self.buf = ""
        self.boundary = True
        self.content_indent = None
        self.open_line = line
        self.elements = []  # dicts
        self.stmt = None
        self.close_line = None


def classify(buf):
    b = " ".join(buf.split())
    if b.endswith("!") or "macro_rules!" in b:
        return "macro", False
    if ITEM_RE.match(b):
        return "item", False
    if FN_RE.search(b):
        return "fn", True
    if "=>" in b:
        b = b.rsplit("=>", 1)[1].strip()
        if not b:
            return "block", True
    if b.endswith("|") or re.search(r"\|\s*->\s*[\w:<>,\s&'()\[\]]+$", b):
        return "closure", False
    if re.search(r"(?<!\w)async(\s+move)?$", b):
        return "async", False
    if re.search(r"(?<!\w)(else|loop|unsafe|try|const)$", b) or b == "" or b.endswith("=") and not b.endswith("=="):
        return "block", True
    if b.endswith(("return", "(", "[", ",")):
        return "block", True
    last = None
    for m in CTL_KW.finditer(b):
        last = m
    if last:
        kw = last.group(1)
        rest = b[last.end():]
        if kw in ("if", "while") and re.match(r"\s*let\b", rest) and not re.search(r"(?<![=!<>])=(?![=>])", rest):
            return "pattern", False
        return ("match" if kw == "match" else "block"), True
    return "other", False


def scan(text):
    """Return a list of processed levels with their statement elements."""
    lines = text.split("\n")
    out = []
    root = Level("item", False, False, 0)
    stack = [root]
    i, n, line = 0, len(text), 0
    at_line_start = True
    block_comment = 0

    def line_start(ln):
        top = stack[-1]
        if top.kind not in PROCESSED or top.macro or block_comment:
            return
        raw = lines[ln]
        s = raw.strip()
        if not s:
            return
        indent = len(raw) - len(raw.lstrip())
        if top.content_indent is None:
            top.content_indent = indent
        if not top.boundary or indent != top.content_indent:
            return
        if s.startswith("}") or s.startswith((".", "?", ")", "]", "else")):
            return
        if s.startswith("//") and not s.startswith(("///", "//!")):
            top.elements.append({"type": "comment", "start": ln})
            return
        if s.startswith("#"):
            top.elements.append({"type": "attr", "start": ln})
            return
        st = {"type": "stmt", "start": ln, "text": s, "ctl": False, "semi": False}
        top.elements.append(st)
        top.stmt = st

    while i < n:
        if at_line_start:
            line_start(line)
            at_line_start = False
        c = text[i]
        top = stack[-1]
        if block_comment:
            if text.startswith("*/", i):
                block_comment -= 1
                i += 2
                continue
            if text.startswith("/*", i):
                block_comment += 1
                i += 2
                continue
            if c == "\n":
                line += 1
                at_line_start = True
            i += 1
            continue
        if c == "\n":
            line += 1
            at_line_start = True
            i += 1
            continue
        if c in " \t\r":
            i += 1
            continue
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if text.startswith("/*", i):
            block_comment = 1
            i += 2
            continue
        prev = text[i - 1] if i else " "
        m = re.match(r'(b?r)(#*)"', text[i:i + 40]) if not (prev.isalnum() or prev == "_") else None
        if m or c == '"' or (c == "b" and text.startswith('b"', i) and not (prev.isalnum() or prev == "_")):
            if m:
                hashes = m.group(2)
                end = text.find('"' + hashes, i + m.end())
                end = n if end < 0 else end + 1 + len(hashes)
            else:
                j = text.index('"', i) + 1
                while j < n and text[j] != '"':
                    j += 2 if text[j] == "\\" else 1
                end = j + 1
            line += text.count("\n", i, end)
            top.buf += '""'
            top.boundary = False
            i = end
            continue
        if c == "'":
            if i + 1 < n and text[i + 1] == "\\":
                j = text.index("'", i + 2)
                while text[j - 1] == "\\" and text[j - 2] != "\\":
                    j = text.index("'", j + 1)
                i = j + 1
                top.buf += "'c'"
                top.boundary = False
                continue
            if i + 2 < n and text[i + 2] == "'":
                i += 3
                top.buf += "'c'"
                top.boundary = False
                continue
            top.buf += c
            top.boundary = False
            i += 1
            continue
        if c in "{([":
            if c == "{":
                kind, ctl = classify(top.buf)
            else:
                kind, ctl = ("macro", False) if top.buf.rstrip().endswith("!") else ("paren", False)
            macro = top.macro or kind == "macro"
            if kind == "pattern":
                kind = "other"
            elif c == "{":
                top.buf = ""
            lvl = Level(kind, ctl, macro, line)
            lvl.owner = top.stmt if c == "{" and ctl else None
            stack.append(lvl)
            top.boundary = False
            i += 1
            continue
        if c in "})]":
            if len(stack) > 1:
                lvl = stack.pop()
                lvl.close_line = line
                # Only a block that spans lines makes its statement compound.
                if getattr(lvl, "owner", None) is not None and line > lvl.open_line:
                    lvl.owner["ctl"] = True
                if lvl.kind in PROCESSED and not lvl.macro:
                    out.append(lvl)
            top = stack[-1]
            if c == "}":
                top.buf = ""
                top.boundary = True
            else:
                top.buf += "()"
                top.boundary = False
            i += 1
            continue
        if c == ";":
            if top.stmt is not None:
                top.stmt["semi"] = True
            top.buf = ""
            top.boundary = True
            i += 1
            continue
        if c == "," and top.kind in ("other", "match", "paren"):
            top.buf = ""
            i += 1
            continue
        m = re.match(r"\w+|=>|::|->|\S", text[i:i + 200])
        tok = m.group(0)
        top.buf += (" " if tok[0].isalnum() or tok[0] == "_" else "") + tok
        top.boundary = False
        i += len(tok)
    return out, lines


def wanted_blanks(levels, lines):
    """Line numbers before which a blank line is required."""
    need = set()
    for lvl in levels:
        units, cur = [], None
        for el in lvl.elements:
            if cur is None:
                cur = {"start": el["start"], "lead": el["type"], "stmt": None}
            if el["type"] == "stmt":
                cur["stmt"] = el
                units.append(cur)
                cur = None
        if cur is not None:
            continue  # trailing comment without statement: leave alone
        nstmts = len(units)
        for idx in range(1, nstmts):
            p, u = units[idx - 1], units[idx]
            ps, us = p["stmt"], u["stmt"]
            ucomp = us["ctl"]
            pcomp = ps["ctl"]
            want = False
            if u["lead"] == "comment" or ucomp or pcomp:
                want = True
            if JUMP_RE.match(us["text"]) and nstmts > 2:
                want = True
            if (
                lvl.kind == "fn"
                and idx == nstmts - 1
                and idx >= 2
                and not us["semi"]
                and not JUMP_RE.match(us["text"])
            ):
                want = True
            # Rule 7: a single-line let checked by the next if / match stays adjacent.
            if (
                want
                and u["lead"] != "comment"
                and ps["text"].startswith("let ")
                and ps["semi"]
                and not pcomp
                and ps["start"] == u["start"] - 1
                and re.match(r"^(if|match)\b|^let\b[^=]*=\s*(if|match)\b", us["text"])
            ):
                pat = ps["text"][4:].split("=", 1)[0].split(":", 1)[0]
                names = set(LET_NAMES.findall(pat)) - {"mut", "ref", "_"}
                if any(re.search(r"\b%s\b" % re.escape(nm), us["text"]) for nm in names):
                    want = False
            if want and lines[u["start"] - 1].strip():
                need.add(u["start"])
    return need


def process(path, fix):
    text = path.read_text()
    levels, lines = scan(text)
    need = wanted_blanks(levels, lines)
    if fix and need:
        new = []
        for ln, s in enumerate(lines):
            if ln in need:
                new.append("")
            new.append(s)
        path.write_text("\n".join(new))
    return need


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--check", action="store_true", help="report violations and exit 1 if any")
    g.add_argument("--fix", action="store_true", help="insert the missing blank lines")
    ap.add_argument("--exclude", action="append", default=[], help="path to skip (repeatable)")
    ap.add_argument("paths", nargs="*", type=Path)
    args = ap.parse_args()
    # The Rust workspace, wherever the script is run from.
    roots = args.paths or [Path(__file__).resolve().parents[2] / "src" / "forge"]
    files = []
    for r in roots:
        files += sorted(r.rglob("*.rs")) if r.is_dir() else [r]
    skip = {Path(e).resolve() for e in args.exclude}
    total = 0
    for f in files:
        if f.resolve() in skip or "target" in f.parts:
            continue
        need = process(f, args.fix)
        total += len(need)
        if args.check:
            for ln in sorted(need):
                print(f"{f}:{ln + 1}: missing blank line before this line")
    verb = "inserted" if args.fix else "missing"
    print(f"{total} blank line(s) {verb}", file=sys.stderr)
    return 1 if args.check and total else 0


if __name__ == "__main__":
    sys.exit(main())
