// Agents write maths three ways: `$x$` and `$$x$$` (Claude), `\(x\)` and `\[x\]` (GPT models).
// remark-math reads `$$…$$` only (inline on a line, or a block on lines of its own), so a lone `$`
// stays a dollar: "costs $5 and $10" or "$PATH" mustn't turn into maths. This rewrites the other
// forms into `$$`, leaving code alone.

/** `$…$` counts as maths the way Pandoc decides: no space just inside either `$`, and no digit
 * right after the closing one. */
const SINGLE = /(^|[^\\$])\$(?!\$)(?=\S)([^\n$]*?[^\s\\$])\$(?!\$)(?!\d)/g;

function text(chunk: string): string {
  // Code spans keep their dollars and backslashes: set them aside while rewriting.
  const kept: string[] = [];
  const out = chunk
    .replace(/(`+)[^`]*?\1/g, (code) => `\u0000${kept.push(code) - 1}\u0000`)
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, m: string) => `\n$$\n${m.trim()}\n$$\n`)
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, m: string) => `$$${m.trim()}$$`)
    .replace(SINGLE, (_, before: string, m: string) => `${before}$$${m}$$`);
  return out.replace(/\u0000(\d+)\u0000/g, (_, i: string) => kept[Number(i)]);
}

/** The source with every kind of maths delimiter as remark-math reads it. */
export function mathDelimiters(source: string): string {
  if (!/[$\\]/.test(source)) return source;
  // Fenced code blocks are left as they are.
  const parts = source.split(/(^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?(?:^ {0,3}\2[^\n]*$|(?![\s\S])))/m);
  let out = "";
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    if (i % 3 === 0) out += text(p);
    else if (i % 3 === 1) out += p;
    // i % 3 === 2: the fence characters captured inside the block, already included.
  }
  return out;
}
