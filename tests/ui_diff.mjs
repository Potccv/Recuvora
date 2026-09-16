import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

// No HTML parser is available: all external text must survive as text nodes.
class TextNode {
  constructor(tag = "#text", text = "") { this.tag = tag; this.text = text; this.children = []; this.attributes = {}; this.dataset = {}; this.listeners = {}; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  setAttribute(name, value) { this.attributes[name] = value; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  get textContent() { return this.text + this.children.map((child) => child.textContent ?? String(child)).join(""); }
}
const context = vm.createContext({
  Node: TextNode,
  TextEncoder,
  document: { createElement: (tag) => new TextNode(tag), createTextNode: (text) => new TextNode("#text", text) },
});
const shell = new vm.SyntheticModule(["copyPlainText"], function () { this.setExport("copyPlainText", async () => {}); }, { context });
const source = await readFile(new URL("../src/interfaces/ui/public/dom.js", import.meta.url), "utf8");
const dom = new vm.SourceTextModule(source, { context });
await dom.link((specifier) => { assert.equal(specifier, "./shell.js"); return shell; });
await dom.evaluate();
const { lineDiff, diffViewer, el } = dom.namespace;

function checkReconstruction(before, after) {
  const diff = lineDiff(before, after);
  assert.equal(diff.rows.filter((row) => row.kind !== "add").map((row) => row.raw).join(""), before, "difference lost or reordered original text");
  assert.equal(diff.rows.filter((row) => row.kind !== "remove").map((row) => row.raw).join(""), after, "difference lost or reordered replacement text");
  let oldLine = 1;
  let newLine = 1;
  for (const row of diff.rows) {
    assert.equal(row.before, row.kind === "add" ? null : oldLine++);
    assert.equal(row.after, row.kind === "remove" ? null : newLine++);
  }
  return diff;
}

for (const [before, after] of [["", ""], ["", "x\n"], ["x\n", ""], ["a\nb\nc\n", "a\nchanged\nc\n"], ["x\nx\ny\nx\n", "x\ny\nx\nx\n"], ["x\r\n", "x\n"], ["x\n", "x"], ["\n\n", "\n"], ["x\rx\n", "x\rx\n"]]) checkReconstruction(before, after);
const edit = checkReconstruction("a\nb\nc\n", "a\nchanged\nc\n");
assert.equal(edit.added, 1);
assert.equal(edit.removed, 1);
assert.equal(lineDiff("same\n", "same\n").added, 0);
assert.ok(lineDiff("x\n", "x").rows.some((row) => row.kind === "add" && row.ending === "none"), "final newline difference was hidden");

let seed = 20260916;
const random = () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed; };
const tokens = ["a\n", "b\n", "a\n", "\n", "same\r\n", "<script>external()</script>\n"];
for (let run = 0; run < 100; run++) {
  const make = () => Array.from({ length: random() % 28 }, () => tokens[random() % tokens.length]).join("");
  checkReconstruction(make(), make());
}

const largeBefore = "shared\n" + "old\n".repeat(3000) + "last\n";
const largeAfter = "shared\n" + "new\n".repeat(3000) + "last\n";
const large = checkReconstruction(largeBefore, largeAfter);
assert.equal(large.bounded, false, "large changed blocks must avoid an unbounded quadratic table");
assert.equal(large.removed, 3000);
assert.equal(large.added, 3000);

const walk = (node) => [node, ...node.children.flatMap((child) => child instanceof TextNode ? walk(child) : [])];
const malicious = "<img src=x onerror=alert(1)>\n<script>steal()</script>\n";
const viewer = diffViewer({ path: malicious, expected: "old\n", replacement: malicious });
const all = walk(viewer);
assert.ok(all.every((node) => !["script", "img", "iframe"].includes(node.tag)), "external content became an executable element");
assert.ok(viewer.textContent.includes(malicious), "full replacement text was altered");
const buttons = all.filter((node) => node.tag === "button");
const full = all.find((node) => node.className === "diff-grid");
const changes = all.find((node) => node.className === "line-diff");
assert.equal(full.attributes.hidden, "true");
buttons[1].listeners.click();
assert.equal(full.hidden, false);
assert.equal(changes.hidden, true);
assert.equal(buttons[1].attributes["aria-pressed"], "true");
buttons[0].listeners.click();
assert.equal(full.hidden, true);
assert.equal(changes.hidden, false);
const longViewer = diffViewer({ path: "large.txt", expected: largeBefore, replacement: largeAfter });
assert.equal(walk(longViewer).filter((node) => node.className?.startsWith("diff-line diff-line-")).length, 800, "large diffs rendered unbounded DOM rows");
assert.ok(longViewer.textContent.includes(largeAfter), "bounded preview lost the full replacement evidence");
const listener = () => {};
assert.equal(el("button", { on: { click: listener } }).__recuvoraListeners.click, listener);
console.log("File difference checks passed: exact text, line numbers, endings, bounded work, safe rendering, and full-text switching.");
