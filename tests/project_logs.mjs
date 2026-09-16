import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

// Deterministic timers and a text-only DOM exercise the observation-record console lifecycle.
class TestNode {
  constructor(tag = "#text", text = "") { this.tag = tag; this.text = text; this.children = []; this.attributes = {}; this.dataset = {}; this.listeners = {}; this.scrollTop = 0; this.scrollHeight = 0; this.clientHeight = 0; }
  append(...nodes) { for (const node of nodes) { this.children.push(node); if (node instanceof TestNode) node.parent = this; } }
  replaceChildren(...nodes) { for (const child of this.children) if (child instanceof TestNode) child.parent = null; this.children = []; this.text = ""; this.append(...nodes); }
  setAttribute(key, value) { this.attributes[key] = value; if (key.startsWith("data-")) this.dataset[key.slice(5).replace(/-([a-z])/g, (_match, letter) => letter.toUpperCase())] = String(value); }
  addEventListener(name, listener) { this.listeners[name] = listener; }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this); this.parent = null; }
  get isConnected() { return Boolean(this.mounted || this.parent?.isConnected); }
  get textContent() { return this.text + this.children.map((child) => child instanceof TestNode ? child.textContent : String(child)).join(""); }
  set textContent(value) { this.replaceChildren(); this.text = String(value); }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) {
    const data = selector.match(/^\[data-([a-z-]+)\]$/);
    const matches = (node) => data ? node.dataset[data[1].replace(/-([a-z])/g, (_match, letter) => letter.toUpperCase())] !== undefined : node.tag === selector;
    return this.children.flatMap((child) => child instanceof TestNode ? [...(matches(child) ? [child] : []), ...child.querySelectorAll(selector)] : []);
  }
}
const timers = new Map();
let timerId = 0;
const documentListeners = new Map();
const document = {
  hidden: false,
  createElement: (tag) => new TestNode(tag),
  createElementNS: (_namespace, tag) => new TestNode(tag),
  createTextNode: (text) => new TestNode("#text", text),
  addEventListener: (name, listener) => documentListeners.set(name, listener),
  removeEventListener: (name, listener) => { if (documentListeners.get(name) === listener) documentListeners.delete(name); },
};
const context = vm.createContext({ Node: TestNode, document, AbortController, TextEncoder, TextDecoder, URL, URLSearchParams,
  setTimeout: (callback, delay) => { const id = ++timerId; timers.set(id, { callback, delay }); return id; }, clearTimeout: (id) => timers.delete(id),
});
const publicRoot = new URL("../src/interfaces/ui/public/", import.meta.url);
const modules = new Map();
const shell = new vm.SyntheticModule(["copyPlainText"], function () { this.setExport("copyPlainText", async () => {}); }, { context });
async function load(name) {
  if (name === "./shell.js") return shell;
  if (!modules.has(name)) modules.set(name, new vm.SourceTextModule(await readFile(new URL(name, publicRoot), "utf8"), { context, identifier: name }));
  return modules.get(name);
}
const module = await load("project-logs.js");
await module.link(load);
await module.evaluate();
const { ProjectLogsConsole } = module.namespace;
const { ApiError } = modules.get("./api.js").namespace;
const flush = async () => { for (let count = 0; count < 12; count++) await Promise.resolve(); };
async function step() {
  assert.ok(timers.size, "expected a scheduled log read");
  const [id, timer] = timers.entries().next().value;
  timers.delete(id);
  timer.callback();
  await flush();
}
const malicious = "<script>external()</script>\n<img src=x onerror=external()>";
const entry = (id, message = malicious) => ({ id: String(id), timestamp: 1000, level: "ERROR", message });
const response = (items = [], cursor = "cursor-1", errors = []) => ({ items, errors, next_cursor: cursor, has_more: false, read_at: 2000, stream_label: "stream-a", target_id: "target-a", coverage: "complete" });
let permits = true;
let next = () => Promise.resolve(response([entry("one")], "cursor-1", [entry("one")]));
const calls = [];
const api = { configured: true, projectLogs: (id, options) => { calls.push({ id, options }); return next(id, options); } };
const consoleView = new ProjectLogsConsole({ api, can: () => permits, isDemo: () => false });
const root = consoleView.render("monitor-target");
root.mounted = true;
const unrelated = new TestNode("textarea");
unrelated.value = "unfinished draft";
consoleView.activate("monitor-target");
await step();
assert.equal(calls.length, 1);
assert.equal(calls[0].options.cursor, "", "entering a page did not request the bounded tail");
assert.equal(calls[0].options.limit, 32);
assert.equal(consoleView.items.length, 1);
assert.equal(consoleView.errors.length, 1);
assert.ok(root.textContent.includes(malicious));
assert.ok(root.textContent.includes("观测记录 · target-a"), "the page did not identify its actual monitored target");
assert.equal(root.querySelectorAll("script").length + root.querySelectorAll("img").length, 0, "log text became executable markup");
assert.equal(unrelated.value, "unfinished draft", "log reading changed an unrelated draft");
const originalRow = consoleView.logStream.children[0];
await step();
assert.equal(consoleView.items.length, 1, "a duplicate event was appended");
assert.equal(consoleView.logStream.children[0], originalRow, "unchanged log entries were recreated");
assert.equal(calls.at(-1).options.cursor, "cursor-1", "incremental reading lost its cursor");
const placeholder = consoleView.render("monitor-target");
assert.equal(placeholder.dataset.liveRegion, "project-logs");
assert.equal(placeholder.children.length, 0, "a global snapshot renderer created another live log region");

consoleView.toggleLive();
assert.equal(timers.size, 0, "pausing left a read scheduled");
consoleView.activate("monitor-target");
assert.equal(consoleView.paused, true, "global snapshot activation overrode the user's pause");
consoleView.toggleLive();
let finish;
next = (_id, { signal }) => new Promise((resolve) => { finish = () => resolve(response([entry("cancelled")], "old-cursor")); assert.equal(signal.aborted, false); });
await step();
const pendingCount = calls.length;
consoleView.toggleLive();
assert.equal(calls.at(-1).options.signal.aborted, true, "pause did not cancel its read");
consoleView.toggleLive();
await step();
assert.equal(calls.length, pendingCount, "a replacement read overlapped a cancelling read");
finish();
await flush();
assert.equal(consoleView.items.some((item) => item.id === "cancelled"), false, "a cancelled response changed the visible logs");
next = () => Promise.resolve(response([entry("two")], "cursor-2"));
await step();
assert.equal(calls.at(-1).options.cursor, "cursor-1", "resume discarded the last accepted cursor");

document.hidden = true;
documentListeners.get("visibilitychange")();
assert.equal(timers.size, 0, "hidden pages kept reading");
document.hidden = false;
documentListeners.get("visibilitychange")();
next = () => Promise.reject(new ApiError("offline", "connection interrupted"));
await step();
assert.equal(consoleView.items.length, 2, "offline reading discarded previously accepted logs");
assert.ok([...timers.values()][0].delay > 2000, "offline reading had no backoff");
assert.ok(root.textContent.includes("数据可能陈旧"));
next = () => Promise.reject(new ApiError("logs_read_failed", "provider temporarily offline", { status: 503 }));
await step();
assert.equal(consoleView.stopped, false, "a temporary provider failure stopped real-time reading permanently");
assert.ok(timers.size, "temporary provider failures were not scheduled for a bounded retry");
next = () => Promise.reject(new ApiError("cursor_invalid", "rotated", { status: 409 }));
await step();
assert.equal(consoleView.stopped, true);
assert.equal(timers.size, 0, "invalid cursors were automatically retried");
next = () => Promise.resolve(response([entry("latest")], "new-cursor"));
consoleView.restartStream();
await step();
assert.equal(calls.at(-1).options.cursor, "", "explicit latest-log reading did not reset the cursor");
assert.equal(consoleView.items[0].id, "latest");

let page = 0;
next = () => { const ids = Array.from({ length: 32 }, (_value, index) => entry(`bounded-${page * 32 + index}`, "x".repeat(2000))); page++; return Promise.resolve(response(ids, `bounded-cursor-${page}`, ids)); };
for (let count = 0; count < 100; count++) await step();
assert.ok(consoleView.items.length <= 2000 && consoleView.errors.length <= 400, "retained logs grew without bounds");
assert.ok(consoleView.logStream.children.length <= 2000 && consoleView.errorStream.children.length <= 400, "rendered logs grew without bounds");
assert.ok(consoleView.items.reduce((sum, item) => sum + item.message.length, 0) <= 2 * 1024 * 1024, "retained text grew without bounds");
consoleView.deactivate();
assert.equal(timers.size, 0);
assert.equal(documentListeners.size, 0, "leaving the page leaked its visibility listener");
assert.equal(consoleView.render("monitor-target"), root, "route remounting returned an empty live-region placeholder");
const exitCalls = calls.length;
permits = false;
consoleView.activate("monitor-target");
await step();
assert.equal(calls.length, exitCalls, "permission loss allowed another log read");
assert.equal(consoleView.items.length + consoleView.errors.length + consoleView.seen.size, 0, "permission loss retained target logs");
consoleView.clear();
assert.equal(consoleView.items.length + consoleView.errors.length + consoleView.seen.size, 0, "clearing an identity retained target logs");
assert.equal(timers.size, 0);
console.log("Observation record checks passed: bounded tail, incremental cursors, safe text, deduplication, local updates, pause/resume, cancellation, visibility, offline backoff, explicit stream recovery, permissions, and cleanup.");
