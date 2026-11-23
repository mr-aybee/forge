// A small line diff (LCS) for comparing compose revisions; no dependencies.

export interface DiffLine {
  op: "same" | "add" | "del";
  text: string;
  /** Line number in the old (`del`/`same`) or new (`add`/`same`) text. */
  a?: number;
  b?: number;
}

/** Lines of `a` → `b`. Common prefix/suffix are trimmed before the O(n·m) table, which is capped. */
export function diffLines(a: string, b: string, maxCells = 4_000_000): DiffLine[] {
  const x = a.split("\n");
  const y = b.split("\n");
  let start = 0;
  while (start < x.length && start < y.length && x[start] === y[start]) start++;
  let endX = x.length;
  let endY = y.length;
  while (endX > start && endY > start && x[endX - 1] === y[endY - 1]) {
    endX--;
    endY--;
  }
  const out: DiffLine[] = [];
  for (let i = 0; i < start; i++) out.push({ op: "same", text: x[i] ?? "", a: i + 1, b: i + 1 });

  const xs = x.slice(start, endX);
  const ys = y.slice(start, endY);
  const n = xs.length;
  const m = ys.length;
  if (n * m > maxCells) {
    // Too big to align: show the middle as removed then added.
    xs.forEach((t, i) => out.push({ op: "del", text: t, a: start + i + 1 }));
    ys.forEach((t, j) => out.push({ op: "add", text: t, b: start + j + 1 }));
  } else {
    // lcs[i][j] = LCS length of xs[i..] and ys[j..], in one flat array.
    const w = m + 1;
    const lcs = new Uint32Array((n + 1) * w);
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        lcs[i * w + j] = xs[i] === ys[j] ? (lcs[(i + 1) * w + j + 1] ?? 0) + 1 : Math.max(lcs[(i + 1) * w + j] ?? 0, lcs[i * w + j + 1] ?? 0);
    let i = 0;
    let j = 0;
    while (i < n && j < m) {
      if (xs[i] === ys[j]) {
        out.push({ op: "same", text: xs[i] ?? "", a: start + i + 1, b: start + j + 1 });
        i++;
        j++;
      } else if ((lcs[(i + 1) * w + j] ?? 0) >= (lcs[i * w + j + 1] ?? 0)) {
        out.push({ op: "del", text: xs[i] ?? "", a: start + i + 1 });
        i++;
      } else {
        out.push({ op: "add", text: ys[j] ?? "", b: start + j + 1 });
        j++;
      }
    }
    for (; i < n; i++) out.push({ op: "del", text: xs[i] ?? "", a: start + i + 1 });
    for (; j < m; j++) out.push({ op: "add", text: ys[j] ?? "", b: start + j + 1 });
  }
  for (let k = 0; k < x.length - endX; k++) out.push({ op: "same", text: x[endX + k] ?? "", a: endX + k + 1, b: endY + k + 1 });
  return out;
}
