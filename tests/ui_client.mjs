import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { webcrypto } from "node:crypto";
import vm from "node:vm";

const publicRoot = new URL("../src/interfaces/ui/public/", import.meta.url);
const apiModule = new vm.SourceTextModule(await readFile(new URL("api.js", publicRoot), "utf8"));
await apiModule.link(() => { throw new Error("The HTTP transport must remain dependency-free"); });
await apiModule.evaluate();
const { ApiClient, ApiError, normalizeBaseUrl, pollSnapshots } = apiModule.namespace;
const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const nativeFetch = globalThis.fetch;
globalThis.fetch = function () { assert.equal(this, globalThis, "native fetch lost its browser receiver"); return Promise.resolve(json({})); };
const defaultTransport = new ApiClient();
globalThis.fetch = nativeFetch;
defaultTransport.connect("http://localhost:8787", "transport-test-token");
await defaultTransport.bootstrap();
defaultTransport.disconnect();
const calls = [];
let response = () => json({});
const client = new ApiClient({ fetchImpl: async (url, options) => { calls.push({ url, options }); return response(url, options); }, timeoutMs: 50, maxBytes: 1024 });

assert.equal(normalizeBaseUrl("http://127.0.0.1:8787/"), "http://127.0.0.1:8787");
for (const url of ["http://example.com", "http://127.0.0.1.example.com", "https://x:y@example.com", "https://example.com/path", "https://example.com/?token=x", "file:///tmp/"]) {
  assert.throws(() => normalizeBaseUrl(url), ApiError);
}
await assert.rejects(client.bootstrap(), (error) => error.code === "unconfigured");
assert.equal(calls.length, 0);
client.connect("http://localhost:8787", "memory-only-token");
await client.bootstrap();
assert.equal(calls[0].options.headers.Authorization, "Bearer memory-only-token");
assert.equal(calls[0].options.credentials, "omit");
assert.equal(calls[0].options.redirect, "error");
assert.equal(JSON.stringify(client).includes("memory-only-token"), false);

response = () => { throw new TypeError("connection lost"); };
let before = calls.length;
await assert.rejects(client.submit("/api/v1/repairs/runs", { operation_id: "ui-known-id", prompt: "x" }), (error) => error.unknown && !error.autoRetry);
assert.equal(calls.length, before + 1, "uncertain POST was retried");
response = () => json({ error: { code: "revision_conflict", message: "current revision is 3" }, auto_retry: false }, 409);
before = calls.length;
await assert.rejects(client.submit("/api/v1/approvals/request/approve", { revision: 1, reason: "reviewed" }), (error) => error.status === 409 && !error.unknown && !error.autoRetry);
assert.equal(calls.length, before + 1, "conflicting approval was retried");
response = () => json({ error: { code: "conversation_outcome_unknown", message: "thread may exist" }, auto_retry: false }, 502);
await assert.rejects(client.submit("/api/v1/harness/id/runs", {}), (error) => error.unknown && error.details.error.code === "conversation_outcome_unknown");
response = () => new Response("<html>not JSON</html>", { status: 202, headers: { "Content-Type": "text/html" } });
await assert.rejects(client.submit("/api/v1/simulations", {}), (error) => error.unknown);
response = () => json({ value: "x".repeat(2048) });
await assert.rejects(client.bootstrap(), (error) => error.code === "response_too_large");

const malicious = "<img src=x onerror=alert(1)>\n<script>steal()</script>";
response = () => json({ items: [{ id: "1", level: "warning", source: "model", timestamp: 1, message: malicious }], next_cursor: "page & 2" });
const logs = await client.logs({ query: "a&b?x", task_id: "task/1", cursor: "page & 1", limit: 25, ignored: "no" });
assert.equal(logs.items[0].message, malicious);
const logUrl = new URL(calls.at(-1).url);
assert.equal(logUrl.searchParams.get("query"), "a&b?x");
assert.equal(logUrl.searchParams.get("cursor"), "page & 1");
assert.equal(logUrl.searchParams.has("ignored"), false);
response = () => json({ plugin: {} });
await client.pluginMonitoring("plugin one");
assert.equal(new URL(calls.at(-1).url).pathname, "/api/v1/monitoring/plugins/plugin%20one");
assert.equal(calls.at(-1).options.method, "GET");
assert.equal(calls.at(-1).options.body, undefined);

response = () => json({ schema_version: 1 });
before = calls.length;
let snapshots = 0;
await pollSnapshots(client, { intervalMs: 0, maxPolls: 3, onSnapshot: () => snapshots++, onError: (error) => { throw error; } });
assert.equal(snapshots, 3);
assert.equal(calls.length, before + 3);
const cancel = new AbortController();
await pollSnapshots(client, { signal: cancel.signal, intervalMs: 5000, maxPolls: 10, onSnapshot: () => cancel.abort(), onError: (error) => { throw error; } });
assert.equal(calls.length, before + 4, "cancelled polling continued");
const longPolling = new AbortController();
let longSnapshots = 0;
await pollSnapshots(client, { signal: longPolling.signal, intervalMs: 0, onSnapshot: () => { if (++longSnapshots === 123) longPolling.abort(); }, onError: (error) => { throw error; } });
assert.equal(longSnapshots, 123, "default monitoring stopped at the old 120-snapshot budget");
response = (_url, { signal }) => new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true }));
await assert.rejects(client.submit("/api/v1/simulations", {}), (error) => error.unknown);
client.disconnect();
assert.equal(client.configured, false);
await assert.rejects(client.bootstrap(), (error) => error.code === "unconfigured");

// A minimal DOM exercises the real renderer without launching a browser. It
// deliberately implements text nodes, never HTML parsing or script execution.
class TestNode {
  constructor(tag = "#text", text = "") {
    this.tag = tag; this.text = tag === "#text" ? text : ""; this.children = []; this._attributes = new Map(); this.listeners = {}; this._value = ""; this.parent = null; this.scrollTop = 0; this.scrollHeight = 0; this.clientHeight = 300;
    this.dataset = new Proxy({}, { get: (_object, name) => this.getAttribute(`data-${String(name).replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`) ?? undefined, set: (_object, name, value) => { this.setAttribute(`data-${String(name).replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`, String(value)); return true; } });
    this.classList = { toggle: (name, force) => { const tokens = new Set(this.className.split(/\s+/).filter(Boolean)); const include = force ?? !tokens.has(name); if (include) tokens.add(name); else tokens.delete(name); this.className = [...tokens].join(" "); return include; } };
    if (tag !== "#text" && text) this.append(new TestNode("#text", text));
  }
  get nodeType() { return this.tag === "#text" ? 3 : 1; }
  get nodeName() { return this.tagName; }
  get tagName() { return this.tag === "#text" ? "#text" : this.tag.toUpperCase(); }
  get nodeValue() { return this.nodeType === 3 ? this.text : null; }
  set nodeValue(value) { this.text = String(value); }
  get childNodes() { return this.children; }
  get firstChild() { return this.children[0] ?? null; }
  get parentNode() { return this.parent; }
  get parentElement() { return this.parent; }
  get nextSibling() { return this.parent?.children[this.parent.children.indexOf(this) + 1] ?? null; }
  get attributes() { return [...this._attributes].map(([name, value]) => ({ name, value })); }
  get id() { return this.getAttribute("id") ?? ""; }
  set id(value) { this.setAttribute("id", value); }
  get className() { return this.getAttribute("class") ?? ""; }
  set className(value) { this.setAttribute("class", value); }
  get type() { return this.getAttribute("type") ?? (this.tag === "input" ? "text" : ""); }
  get disabled() { return this.hasAttribute("disabled"); }
  set disabled(value) { if (value) this.setAttribute("disabled", ""); else this.removeAttribute("disabled"); }
  get checked() { return this._checked ?? this.hasAttribute("checked"); }
  set checked(value) { this._checked = Boolean(value); }
  get hidden() { return this.hasAttribute("hidden"); }
  set hidden(value) { if (value) this.setAttribute("hidden", ""); else this.removeAttribute("hidden"); }
  get open() { return this.hasAttribute("open"); }
  set open(value) { if (value) this.setAttribute("open", ""); else this.removeAttribute("open"); }
  get value() { return this._value || (this.tag === "option" ? this.getAttribute("value") ?? this.textContent : this.tag === "select" ? (this.options.find((option) => option.hasAttribute("selected")) ?? this.options[0])?.value ?? "" : ""); }
  set value(value) { this._value = String(value); }
  get isConnected() { return this === root || Boolean(this.parent?.isConnected); }
  append(...nodes) { for (const node of nodes) this.insertBefore(node instanceof TestNode ? node : new TestNode("#text", String(node)), null); }
  insertBefore(node, reference) { if (node === reference) return node; node.remove(); const index = reference ? this.children.indexOf(reference) : this.children.length; assert.ok(index >= 0, `reference ${reference?.className || reference?.nodeName} is not a child of ${this.className || this.nodeName}; inserting ${node.className || node.nodeName}`); this.children.splice(index, 0, node); node.parent = this; return node; }
  replaceChildren(...nodes) { for (const child of [...this.children]) child.remove(); this.append(...nodes); }
  replaceWith(node) { if (this.parent) { const parent = this.parent; parent.insertBefore(node, this); this.remove(); } }
  setAttribute(key, value) { this._attributes.set(key, String(value)); }
  getAttribute(key) { return this._attributes.get(key) ?? null; }
  hasAttribute(key) { return this._attributes.has(key); }
  removeAttribute(key) { this._attributes.delete(key); }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  removeEventListener(name, callback) { if (this.listeners[name] === callback) delete this.listeners[name]; }
  dispatchEvent(event) { this.listeners[event.type]?.({ ...event, target: this, currentTarget: this, preventDefault() {} }); return true; }
  setCustomValidity(message) { this.validityMessage = message; }
  reportValidity() { return !this.validityMessage && this.querySelectorAll("input, textarea, select").every((control) => !control.validityMessage && (!control.hasAttribute("required") || control.value)); }
  focus() { document.activeElement = this; }
  remove() { if (this.parent) { this.parent.children = this.parent.children.filter((child) => child !== this); this.parent = null; } }
  contains(node) { return node === this || this.children.some((child) => child.contains(node)); }
  closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector) ?? null; }
  get options() { return this.children.filter((item) => item.tag === "option"); }
  get textContent() { return this.text + this.children.map((child) => child instanceof TestNode ? child.textContent : String(child)).join(""); }
  set textContent(value) { this.replaceChildren(); if (this.nodeType === 3) this.text = String(value); else { this.text = ""; if (value !== "") this.append(new TestNode("#text", String(value))); } }
  matches(selector) {
    const parts = selector.trim().split(/\s+/);
    const terminal = parts.pop();
    let simple = terminal;
    if (simple.endsWith(":focus")) { if (document.activeElement !== this) return false; simple = simple.slice(0, -6); }
    const attribute = simple.match(/\[([^=\]]+)(?:=["']?([^\]"']+)["']?)?\]/);
    if (attribute && (!this.hasAttribute(attribute[1]) || (attribute[2] !== undefined && this.getAttribute(attribute[1]) !== attribute[2]))) return false;
    simple = simple.replace(/\[[^\]]+\]/g, "");
    const id = simple.match(/#([\w-]+)/); if (id && this.id !== id[1]) return false;
    const classes = [...simple.matchAll(/\.([\w-]+)/g)].map((match) => match[1]);
    if (classes.some((name) => !this.className.split(/\s+/).includes(name))) return false;
    const tag = simple.match(/^[a-zA-Z][\w-]*/)?.[0]; if (tag && this.tagName !== tag.toUpperCase()) return false;
    if (!parts.length) return this.nodeType === 1;
    let parent = this.parent;
    while (parent && !parent.matches(parts.join(" "))) parent = parent.parent;
    return Boolean(parent);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) {
    const matches = (node) => selector.split(",").some((part) => node.matches(part));
    return this.children.flatMap((child) => child instanceof TestNode ? [...(matches(child) ? [child] : []), ...child.querySelectorAll(selector)] : []);
  }
}
const root = new TestNode("body");
const app = new TestNode("div"); app.id = "app";
const announcer = new TestNode("div"); announcer.id = "route-announcer";
root.append(app, announcer);
const documentListeners = {};
const windowListeners = {};
const document = {
  hidden: true,
  activeElement: null,
  documentElement: new TestNode("html"),
  createElement: (tag) => new TestNode(tag),
  createElementNS: (_namespace, tag) => new TestNode(tag),
  createTextNode: (text) => new TestNode("#text", text),
  getElementById: (id) => root.querySelector(`#${id}`),
  querySelector: (selector) => root.querySelector(selector),
  querySelectorAll: (selector) => root.querySelectorAll(selector),
  addEventListener: (name, callback) => { documentListeners[name] = callback; },
  removeEventListener: (name, callback) => { if (documentListeners[name] === callback) delete documentListeners[name]; },
};
const uiCalls = [];
let uiResponse = () => json({});
const timers = new Set();
const location = { hash: "#/overview", protocol: "http:", origin: "http://localhost:8787" };
const clipboardCopies = [];
const context = vm.createContext({
  document, location, Node: TestNode, navigator: { clipboard: { async writeText(value) { clipboardCopies.push(value); } } }, history: { replaceState(_state, _unused, hash) { location.hash = hash; } },
  window: { addEventListener(name, callback) { windowListeners[name] = callback; }, scrollTo() {} }, requestAnimationFrame: (callback) => callback(),
  URL, URLSearchParams, TextEncoder, TextDecoder, AbortController, Event, crypto: webcrypto, console,
  setTimeout: (callback, delay) => { const timer = setTimeout(callback, delay); timers.add(timer); return timer; },
  clearTimeout: (timer) => { clearTimeout(timer); timers.delete(timer); },
  fetch: async (url, options) => { uiCalls.push({ url, options }); return uiResponse(url, options); },
});
const modules = new Map();
const moduleLoads = new Map();
async function loadModule(name) {
  if (moduleLoads.has(name)) return moduleLoads.get(name);
  const pending = (async () => {
    let source = await readFile(new URL(name, publicRoot), "utf8");
  if (name === "app.js") source += "\nexport { api, connection, selectDemo, applySnapshot, renderApp, renderIfIdle, renderLogs, renderSimulation, copyable, logView, operationAction, operationViews, approvalActionBar, startPolling, historyData, renderHistoryList, renderApprovalDetail, renderRepairDetail, monitoring, authenticateConnection, requireAuthentication, drafts };";
    const loaded = new vm.SourceTextModule(source, { context, identifier: name });
    modules.set(name, loaded);
    return loaded;
  })();
  moduleLoads.set(name, pending);
  return pending;
}
const module = await loadModule("app.js");
await module.link((specifier) => loadModule(specifier.replace(/^\.\//, "")));
await module.evaluate();
const ui = module.namespace;
const pluginUi = modules.get("plugin-monitoring.js").namespace;
const minimalPluginView = { schema_version: 1, title: "Plugin", sections: [{ id: "main", title: "Main", monitor_role: "snapshot", fields: [{ id: "name", label: "Name", source: "last_value", pointer: "/name", format: "text" }] }] };
assert.equal(pluginUi.validatePluginView(minimalPluginView).sections[0].fields[0].pointer, "/name");
for (const invalidView of [
  { ...minimalPluginView, unknown: true },
  { ...minimalPluginView, title: `bad\0title` },
  { ...minimalPluginView, summary: "x".repeat(33 * 1024) },
  { ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: [{ ...minimalPluginView.sections[0].fields[0], pointer: "/bad~2escape" }] }] },
  { ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: [{ ...minimalPluginView.sections[0].fields[0], pointer: "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q" }] }] },
  { ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: [] }] },
  { ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: [minimalPluginView.sections[0].fields[0], { ...minimalPluginView.sections[0].fields[0] }] }] },
  { ...minimalPluginView, sections: [minimalPluginView.sections[0], { ...minimalPluginView.sections[0], id: "second", fields: [{ ...minimalPluginView.sections[0].fields[0] }] }] },
]) assert.throws(() => pluginUi.validatePluginView(invalidView), (error) => error.name === "ApiError" && error.code === "invalid_response");
assert.throws(() => pluginUi.validatePluginView({ ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: Array.from({ length: 65 }, (_, index) => ({ ...minimalPluginView.sections[0].fields[0], id: `field-${index}` })) }] }), (error) => error.name === "ApiError" && error.code === "invalid_response");
assert.equal(pluginUi.knownState(pluginUi.viewRuntimeStates, "future_state").label, "状态未知");
const unsafeNumericView = pluginUi.validatePluginView({ ...minimalPluginView, sections: [{ ...minimalPluginView.sections[0], fields: [
  { id: "number", label: "Number", source: "last_value", pointer: "/number", format: "number", empty: "number rejected" },
  { id: "timestamp", label: "Timestamp", source: "last_value", pointer: "/timestamp", format: "timestamp_ms", empty: "timestamp rejected" },
  { id: "duration", label: "Duration", source: "last_value", pointer: "/duration", format: "duration_ms", empty: "duration rejected" },
  { id: "json", label: "JSON", source: "last_value", pointer: "/nested", format: "json", empty: "json rejected" },
] }] });
const unsafeNumericOutput = pluginUi.renderPluginView(unsafeNumericView, [{ target_id: "target", view_role: "snapshot", last_value: {
  number: Number.MAX_SAFE_INTEGER + 1,
  timestamp: 1.5,
  duration: Number.MAX_SAFE_INTEGER + 1,
  nested: { id: Number.MAX_SAFE_INTEGER + 1 },
} }]);
assert.match(unsafeNumericOutput.textContent, /number rejected/);
assert.match(unsafeNumericOutput.textContent, /timestamp rejected/);
assert.match(unsafeNumericOutput.textContent, /duration rejected/);
assert.match(unsafeNumericOutput.textContent, /json rejected/);
const navigate = (hash) => { location.hash = hash; windowListeners.hashchange(); };
assert.equal(ui.connection.mode, "live");
assert.match(app.textContent, /连接 Recuvora/);
assert.equal(app.querySelectorAll(".sidebar").length, 0, "unverified connection exposed the business shell");
assert.equal(document.getElementById("auth-password").type, "password");
assert.equal(app.textContent.includes("fixture-target-alpha"), false, "default live page leaked fixtures");
assert.equal(uiCalls.length, 0, "unconfigured page made a request");
ui.selectDemo(true);
navigate("#/overview");
assert.match(app.textContent, /演示数据/);
ui.selectDemo(false);
assert.equal(app.textContent.includes("fixture-target-alpha"), false, "leaving demo retained fixtures");
const snapshot = { schema_version: 1, mode: "live", runtime: { updatedAt: 1, operator: "test operator", permissions: ["logs.read", "approval.decide", "repair.run"] }, capabilities: [{ id: "text-repair", availability: "available", group: "当前能力", state: "limited", name: "修复" }], harnesses: [], repairs: [], approvals: [], simulation_tasks: [], operations: [], active_configuration: null };
// A failed password never mounts navigation or accepts business fixtures.
navigate("#/capabilities/text-repair");
const authBase = document.getElementById("auth-base");
const authPassword = document.getElementById("auth-password");
authPassword.value = "invalid-test-password";
uiResponse = () => json({ error: { code: "unauthorized", message: "wrong password" } }, 401);
await ui.authenticateConnection(authBase, authPassword);
assert.equal(ui.api.configured, false);
assert.equal(app.querySelectorAll(".sidebar").length, 0);
assert.equal(ui.connection.hasSnapshot, false);
assert.match(app.textContent, /访问密码不正确/);
assert.equal(document.getElementById("auth-password").value, "", "failed authentication retained the submitted password");
// Authentication returns to the deep link the operator originally requested.
document.getElementById("auth-password").value = "valid-test-password";
let finishAuthentication;
uiResponse = () => new Promise((resolve) => { finishAuthentication = resolve; });
const authenticating = ui.authenticateConnection(document.getElementById("auth-base"), document.getElementById("auth-password"));
assert.equal(app.querySelectorAll(".sidebar").length, 0, "pending authentication exposed the business shell");
assert.equal(ui.connection.hasSnapshot, false);
finishAuthentication(json(snapshot));
await authenticating;
assert.equal(location.hash, "#/capabilities/text-repair", "authentication lost the requested record page");
assert.equal(app.querySelectorAll(".sidebar").length, 1);
assert.equal(app.querySelector("h1").textContent, "修复");
ui.historyData.detail("approvals", "private-previous-user").value = { expected: "previous-user-evidence" };
ui.operationViews.set("private-operation", { id: "private-operation", result: { message: "previous-user-result" } });
ui.requireAuthentication(new ApiError("unauthorized", "expired", { status: 401 }));
assert.equal(location.hash, "#/auth");
assert.equal(ui.api.configured, false);
assert.equal(ui.historyData.details.size, 0, "expired authentication retained private evidence");
assert.equal(ui.operationViews.size, 0);
assert.equal(app.querySelectorAll(".sidebar").length, 0);
assert.equal(app.textContent.includes("previous-user-evidence"), false);
uiCalls.length = 0;
ui.applySnapshot(snapshot);
navigate("#/overview");
assert.match(app.textContent, /宿主未提供修复配置/);
navigate("#/settings");
const editedField = document.getElementById("service-base");
editedField.value = "https://operator-chosen.example";
editedField.focus();
const mountedSidebar = document.querySelector(".sidebar");
const mountedShell = document.querySelector(".app-layout");
ui.applySnapshot({ ...snapshot, runtime: { ...snapshot.runtime, updatedAt: 2, operator: "updated operator" } });
ui.renderIfIdle();
assert.equal(document.getElementById("service-base"), editedField, "polling replaced a dirty form after focus moved");
assert.equal(editedField.value, "https://operator-chosen.example", "polling discarded the operator draft");
assert.equal(document.activeElement, editedField, "snapshot update lost keyboard focus");
assert.equal(document.querySelector(".sidebar"), mountedSidebar, "snapshot update replaced navigation");
assert.equal(document.querySelector(".app-layout"), mountedShell, "snapshot update replaced the app shell");
assert.match(mountedSidebar.textContent, /updated operator/, "preserving the form prevented other regions from updating");
assert.match(document.querySelector(".preview-banner").textContent, /真实服务/);
// Dashboard counters continue updating independently and use accurate queues.
navigate("#/overview");
ui.applySnapshot({ ...snapshot, approval_counts: { unknown: 2, waiting_human: 5, approved: 3, pending: 97, executing: 4 } });
ui.renderIfIdle();
const unknownMetric = app.querySelectorAll(".metric-card").find((node) => node.textContent.startsWith("结果未知"));
const approvedMetric = app.querySelectorAll(".metric-card").find((node) => node.textContent.startsWith("批准待执行"));
assert.equal(unknownMetric.querySelector(".metric-value").textContent, "2");
assert.equal(approvedMetric.querySelector(".metric-value").textContent, "3", "waiting-to-apply count included reviews or running execution");
assert.equal(approvedMetric.parentNode.getAttribute("href"), "#/approvals?state=approved");
ui.applySnapshot({ ...snapshot, approval_counts: { unknown: 6, waiting_human: 1, approved: 8, executing: 0 } });
ui.renderIfIdle();
assert.equal(app.querySelectorAll(".metric-card").find((node) => node.textContent.startsWith("结果未知")), unknownMetric, "counter refresh replaced an unchanged card");
assert.equal(unknownMetric.querySelector(".metric-value").textContent, "6");
const repairConfiguration = { targetId: "test-target", targetRoot: "allowed-fixture-root", allowedFiles: ["file.txt"], repairConfig: "fixture-repair-config", executionHarness: "fixture-harness", reviewer: "human", delegation: "replace_text", ttlSeconds: 60, timeoutSeconds: 30, maxToolCalls: 4, dataDirectory: "fixture-data" };
ui.applySnapshot({ ...snapshot, active_configuration: repairConfiguration });
navigate("#/repairs/new");
const repairPrompt = document.getElementById("repair-prompt");
const repairTaskId = document.getElementById("repair-task-id");
repairPrompt.value = "retain my exact repair request";
repairPrompt.dispatchEvent(new Event("input"));
repairTaskId.value = "draft-task-id";
repairTaskId.dispatchEvent(new Event("input"));
repairPrompt.focus();
ui.applySnapshot({ ...snapshot, active_configuration: repairConfiguration, runtime: { ...snapshot.runtime, updatedAt: 3 } });
ui.renderIfIdle();
assert.equal(document.getElementById("repair-prompt"), repairPrompt);
assert.equal(document.activeElement, repairPrompt);
navigate("#/logs");
navigate("#/repairs/new");
assert.equal(document.getElementById("repair-prompt").value, "retain my exact repair request", "visiting logs discarded the repair request draft");
assert.equal(document.getElementById("repair-task-id").value, "draft-task-id");
assert.equal(document.getElementById("repair-prompt").reportValidity(), true, "restored draft still had the empty-field validation error");
ui.applySnapshot(snapshot);
navigate("#/settings");
ui.logView.items = logs.items;
ui.logView.loaded = true;
const logPage = ui.renderLogs();
assert.equal(logPage.querySelectorAll("script").length, 0);
assert.equal(logPage.querySelectorAll("img").length, 0);
assert.equal(logPage.querySelector("pre").textContent, malicious, "log text was interpreted instead of preserved");
assert.equal(ui.copyable(null).querySelectorAll("button").length, 0, "missing session ID exposed a copy action");
const mountedCopy = ui.copyable("old-thread-id");
modules.get("refresh.js").namespace.patchNode(mountedCopy, ui.copyable("new-thread-id"));
assert.equal(mountedCopy.querySelector(".id-value").textContent, "new-thread-id");
await mountedCopy.querySelector("button").listeners.click();
assert.equal(clipboardCopies.at(-1), "new-thread-id", "updated ID display copied the stale captured ID");
ui.applySnapshot({ ...snapshot, simulation_tasks: [{ id: "sim-running", target: "test-target", state: "running", scenario: "succeed", revision: null, duration: "—" }] });
navigate("#/simulation");
const simulationPage = document.getElementById("page-view");
assert.equal(simulationPage.querySelectorAll(".status-chip").some((chip) => chip.textContent === "模拟运行中"), true, `simulation state missing from rendered page: ${simulationPage.textContent}`);
assert.equal(simulationPage.querySelectorAll(".status-chip").some((chip) => chip.textContent === "模拟失败"), false);
assert.equal(simulationPage.textContent.includes("rnull"), false);

ui.api.connect("http://localhost:8787", "test-token");
const allowedApproval = { requestId: "request-safe", revision: 7, allowedActions: ["approve", "reconcile"] };
const approval = ui.approvalActionBar(allowedApproval);
assert.deepEqual(approval.querySelectorAll("button").map((button) => button.textContent), ["批准", "读取并核验当前内容"]);
approval.querySelector("textarea").value = "reviewed exact normalized operation";
uiResponse = () => json({ error: { code: "revision_conflict", message: "new revision" }, auto_retry: false }, 409);
await approval.querySelector("button").listeners.click();
assert.equal(uiCalls.length, 1, "approval conflict retried a side effect");
assert.deepEqual(JSON.parse(uiCalls[0].options.body), { revision: 7, reason: "reviewed exact normalized operation" });
assert.equal(ui.operationViews.values().next().value.status, "failed");

uiResponse = () => { throw new TypeError("disconnect after dispatch"); };
const run = ui.operationAction("run", { permission: "repair.run", available: true, key: "repair", path: "/api/v1/repairs/runs", body: { prompt: "test", task_id: "task" } });
await run.listeners.click();
assert.equal(uiCalls.length, 2, "uncertain submission was repeated by the UI");
const submitted = JSON.parse(uiCalls[1].options.body);
assert.match(submitted.operation_id, /^ui-/);
assert.equal(ui.operationViews.get(submitted.operation_id).status, "unknown");
ui.connection.status = "offline";
const unavailable = ui.operationAction("run", { permission: "repair.run", available: true, key: "repair", path: "/api/v1/repairs/runs", body: {} });
assert.equal(unavailable.disabled, true);
await unavailable.listeners.click();
assert.equal(uiCalls.length, 2, "offline UI dispatched a side effect");

// Page queries replace the current bounded page, preserve filters across polling,
// and leave complete text to explicit detail reads.
const { HistoryStore } = modules.get("history.js").namespace;
const historyCalls = [];
let historyResponse = { items: [{ requestId: "older-pending", revision: 9 }], total: 80, next_cursor: "older-pending", limit: 25 };
const historyClient = {
  list: async (...args) => { historyCalls.push(args); return historyResponse; },
  detail: async (kind, id) => { historyCalls.push([kind, id]); return { requestId: id, revision: 9, expected: malicious, replacement: "reviewed replacement", allowedActions: ["approve"] }; },
};
const historyStore = new HistoryStore(historyClient);
historyStore.seed({ approvals: [{ requestId: "first-page" }], pages: { approvals: { total: 80, next_cursor: "first-page" } } });
await historyStore.load("approvals", { cursor: "first-page", direction: "next" });
assert.equal(historyStore.page("approvals").items[0].requestId, "older-pending");
assert.equal(historyCalls[0][1].state, "attention", "paging lost the attention queue filter");
assert.equal(historyCalls[0][1].cursor, "first-page");
historyStore.seed({ approvals: [{ requestId: "new-first-page" }] });
assert.equal(historyStore.page("approvals").items[0].requestId, "older-pending", "polling overwrote an older history page");
assert.equal(historyStore.page("approvals").items[0].replacement, undefined);
await historyStore.readDetail("approvals", "older-pending");
assert.equal(historyStore.detail("approvals", "older-pending").value.expected, malicious);
historyStore.seed({ approvals: [] });
assert.equal(historyStore.detail("approvals", "older-pending").stale, true, "polling left cached authorization actions fresh");
await historyStore.readDetail("approvals", "older-pending");
assert.equal(historyStore.detail("approvals", "older-pending").stale, false);
for (let index = 0; index < 20; index++) await historyStore.readDetail("approvals", `request-${index}`);
assert.ok(historyStore.details.size <= 8, "full detail cache grew without a bound");
historyResponse = { items: [{ requestId: "filtered-page" }], total: 1, next_cursor: null };
await historyStore.load("approvals", { filters: { state: "all", query: "filtered" } });
historyStore.seed({ approvals: [{ requestId: "new-first-page" }] });
assert.equal(historyStore.page("approvals").items[0].requestId, "filtered-page", "polling discarded a selected history filter");

let resolveLate;
historyClient.list = () => new Promise((resolve) => { resolveLate = resolve; });
const latePage = historyStore.load("operations");
historyStore.clear();
resolveLate({ items: [{ id: "previous-connection" }], total: 1, next_cursor: null });
await latePage;
assert.equal(historyStore.page("operations").items.length, 0, "late response crossed connection identity");

ui.connection.status = "connected";
ui.historyData.detail("approvals", allowedApproval.requestId).stale = true;
const staleApproval = ui.approvalActionBar(allowedApproval);
assert.equal(staleApproval.querySelectorAll("button").length, 0, "stale detail still exposed allowed actions");
// The original button also fails closed after its rendered revision becomes stale.
await approval.querySelector("button").listeners.click();
assert.equal(uiCalls.length, 2, "an old approval button submitted after detail invalidation");

const originalList = ui.api.list;
ui.api.list = async () => ({ items: [{ id: "second-page-record" }], total: 2, next_cursor: null, limit: 25 });
Object.assign(ui.historyData.page("repairs"), { items: [{ id: "first-page-record" }], total: 2, next_cursor: "first-page-record", loaded: true });
const paginated = ui.renderHistoryList("repairs", (items) => new TestNode("p", items.map((item) => item.id).join(",")));
await paginated.querySelectorAll("button").find((button) => button.textContent === "下一页").listeners.click();
assert.match(paginated.textContent, /second-page-record/);
assert.equal(paginated.textContent.includes("first-page-record"), false, "page controls updated the cache without replacing visible rows");
assert.equal(editedField.value, "https://operator-chosen.example", "history reads overwrote an unrelated form draft");
ui.api.list = originalList;

const fullApproval = { requestId: "full-detail", state: "pending", revision: 12, taskId: "task-full", operationId: "op-full", target: "fixture", path: "file.txt", expected: malicious, replacement: "plain replacement", userRequest: "inspect", allowedActions: ["approve"], policy: { id: "policy", version: 1, allowedTargets: ["fixture"], allowedActions: ["replace_text"] }, executionContext: {} };
Object.assign(ui.historyData.detail("approvals", "full-detail"), { value: fullApproval, stale: false });
const detailedPage = ui.renderApprovalDetail("full-detail");
assert.equal(detailedPage.querySelectorAll("pre").some((node) => node.textContent === malicious), true, "detail view did not preserve the complete approval baseline");
assert.equal(detailedPage.querySelectorAll("script").length, 0);
assert.equal(detailedPage.querySelectorAll("img").length, 0);
assert.match(detailedPage.textContent, /revision 12/);
navigate("#/approvals/full-detail");
const decisionReason = document.getElementById("approval-reason");
decisionReason.value = "retain reviewed evidence and decision reasoning";
decisionReason.focus();
const openEvidence = app.querySelector("details");
if (openEvidence) openEvidence.open = true;
const mountedDifference = app.querySelector(".diff-shell");
mountedDifference.querySelectorAll("button").find((button) => button.textContent === "完整文本").listeners.click();
ui.renderIfIdle();
assert.equal(app.querySelector(".diff-shell"), mountedDifference, "unchanged approval update replaced the active evidence view");
assert.equal(mountedDifference.querySelector(".diff-grid").hidden, false, "unchanged update reset full-text selection");
ui.historyData.detail("approvals", fullApproval.requestId).value = { ...fullApproval, revision: 13 };
ui.renderIfIdle();
assert.equal(document.getElementById("approval-reason"), decisionReason, "record update replaced the decision draft control");
assert.equal(document.activeElement, decisionReason);
if (openEvidence) assert.equal(openEvidence.open, true, "record update closed the open evidence panel");
assert.notEqual(app.querySelector(".diff-shell"), mountedDifference, "new approval revision retained an old evidence subtree");
assert.equal(app.querySelector(".diff-shell").querySelector(".diff-grid").hidden, true, "new approval revision did not reset the evidence view");
assert.match(app.textContent, /revision 13/);
navigate("#/logs");
navigate("#/approvals/full-detail");
assert.equal(document.getElementById("approval-reason").value, "retain reviewed evidence and decision reasoning", "visiting logs discarded approval reasoning");
const noToolsRepair = { id: "completed-without-tools", status: "completed", operationCount: 0, target: "fixture", finalResponse: "Analysis complete; no file action was requested.", businessVerified: false };
Object.assign(ui.historyData.detail("repairs", noToolsRepair.id), { value: noToolsRepair, stale: false });
Object.assign(ui.historyData.page("related-approvals"), { items: [], total: 0, loaded: true, filters: { task_id: noToolsRepair.id } });
const noToolsPage = ui.renderRepairDetail(noToolsRepair.id);
assert.match(noToolsPage.textContent, /文件执行事实请核对审批记录/);
assert.match(noToolsPage.textContent, /业务恢复未验证/);
assert.equal(noToolsPage.textContent.includes("写后读回与获准文本一致"), false, "a completed workflow without tools claimed a verified write");
assert.equal(noToolsPage.textContent.includes("文件步骤已结束"), false, "an empty operation list claimed completed file execution");

// The official monitoring page keeps health, sample freshness, collection
// coverage, incident condition, and human acknowledgement independent.
const monitor = { id: "monitor-live", target_id: "target-live", source_id: "source-live", extension_id: "node-live", health: "unknown", freshness: "stale", coverage: "unavailable", running: true, last_received_at_ms: 1000, interval_ms: 5000, last_error: malicious };
const incident = { id: "incident-live", revision: 1, monitor_id: monitor.id, target_id: monitor.target_id, rule_id: "rule-live", kind: "coverage", status: "open", condition: "unknown", summary: "采集失败", evidence: { log: malicious }, first_seen: 1000, last_seen: 2000, resolved_at: null, occurrences: 2, acknowledgement: null, allowed_actions: ["acknowledge"] };
const monitoringSnapshot = { ...snapshot, runtime: { ...snapshot.runtime, permissions: ["monitor.read", "incident.read", "incident.acknowledge"] }, monitoring: { configured: true, monitors: [monitor], runtime_error: null } };
ui.applySnapshot(monitoringSnapshot);
Object.assign(ui.historyData.page("incidents"), { items: [{ ...incident, evidence: undefined }], total: 1, next_cursor: null, loaded: true });
location.hash = "#/monitoring";
ui.renderApp({ focus: false });
assert.match(app.textContent, /健康状态未知/);
assert.match(app.textContent, /样本陈旧/);
assert.match(app.textContent, /无法采集/);
assert.match(app.textContent, /监控采集异常/);
assert.equal(app.querySelectorAll("img").length, 0);
assert.equal(app.querySelectorAll("script").length, 0);
const initialMonitorPosts = uiCalls.filter((call) => call.options.method === "POST").length;
assert.equal(uiCalls.filter((call) => call.url.includes("/incidents/")).length, 0, "incident evidence loaded before a detail read");
uiResponse = () => json(incident);
await ui.monitoring.readDetail("incidents", incident.id);
location.hash = `#/monitoring/incidents/${incident.id}`;
ui.renderApp({ focus: false });
assert.equal(app.querySelectorAll("pre").some((node) => node.textContent.includes(malicious.split("\n")[0])), true);
assert.equal(app.querySelectorAll("img").length, 0);
assert.equal(app.querySelectorAll("script").length, 0);
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, initialMonitorPosts, "read-only monitoring views dispatched an action");

const acknowledgement = ui.monitoring.acknowledgement(incident, ui.historyData.detail("incidents", incident.id));
acknowledgement.querySelector("textarea").value = "已知悉采集异常";
acknowledgement.querySelector("textarea").listeners.input();
const newerIncident = { ...incident, revision: 2, condition: "active" };
uiResponse = (_url, options) => options.method === "POST" ? json({ error: { code: "conflict", message: "current revision is 2" }, auto_retry: false }, 409) : json(newerIncident);
const conflictStart = uiCalls.length;
await acknowledgement.querySelector("button").listeners.click();
assert.deepEqual(JSON.parse(uiCalls[conflictStart].options.body), { revision: 1, note: "已知悉采集异常" });
assert.equal(uiCalls.slice(conflictStart).filter((call) => call.options.method === "POST").length, 1, "acknowledgement conflict retried its POST");
assert.equal(uiCalls.slice(conflictStart).some((call) => call.options.method === "GET" && call.url.endsWith("/incidents/incident-live")), true, "acknowledgement conflict did not reload complete incident detail");
assert.equal(ui.historyData.detail("incidents", incident.id).value.revision, 2);
assert.equal(ui.monitoring.drafts.get(incident.id), "已知悉采集异常");
const afterConflict = uiCalls.length;
await acknowledgement.querySelector("button").listeners.click();
assert.equal(uiCalls.length, afterConflict, "an old incident revision submitted again");

uiResponse = (_url, options) => { if (options.method === "POST") throw new TypeError("lost acknowledgement receipt"); return json(newerIncident); };
const uncertainAck = ui.monitoring.acknowledgement(newerIncident, ui.historyData.detail("incidents", incident.id));
await uncertainAck.querySelector("button").listeners.click();
assert.equal(ui.monitoring.receipts.get(incident.id).status, "unknown");
const uncertainPosts = uiCalls.filter((call) => call.options.method === "POST").length;
await uncertainAck.querySelector("button").listeners.click();
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, uncertainPosts, "unknown acknowledgement was retried");
const acknowledgedIncident = { ...newerIncident, revision: 3, status: "acknowledged", condition: "active", allowed_actions: [], acknowledgement: { actor: "authenticated-operator", note: "已知悉采集异常", at_ms: 3000 } };
let finishOldIncidentRead;
const racingHistory = new HistoryStore({ detail: () => new Promise((resolve) => { finishOldIncidentRead = resolve; }) });
const oldIncidentRead = racingHistory.readDetail("incidents", incident.id);
racingHistory.detail("incidents", incident.id).value = acknowledgedIncident;
finishOldIncidentRead(newerIncident);
await oldIncidentRead;
assert.equal(racingHistory.detail("incidents", incident.id).value.status, "acknowledged", "an older in-flight read overwrote the acknowledgement receipt");
assert.equal(racingHistory.detail("incidents", incident.id).value.revision, 3);
uiResponse = () => json(acknowledgedIncident);
await ui.monitoring.readDetail("incidents", incident.id, true);
// A prior automatic GET may already be in flight; explicitly read its successor.
if (ui.historyData.detail("incidents", incident.id).value.status !== "acknowledged") await ui.monitoring.readDetail("incidents", incident.id, true);
assert.equal(ui.monitoring.receipts.get(incident.id).status, "acknowledged");
assert.equal(ui.historyData.detail("incidents", incident.id).value.condition, "active", "acknowledgement cleared the condition locally");
assert.match(ui.monitoring.renderIncident(incident.id).textContent, /异常条件仍存在/);
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, uncertainPosts);

const resolvedIncident = { ...acknowledgedIncident, revision: 4, status: "resolved", condition: "clear", resolved_at: 4000 };
Object.assign(ui.historyData.detail("incidents", incident.id), { value: resolvedIncident, stale: false });
const resolvedPage = ui.monitoring.renderIncident(incident.id);
assert.match(resolvedPage.textContent, /异常条件已解除/);
assert.match(resolvedPage.textContent, /没有据此验证修复成功或业务恢复/);
assert.equal(resolvedPage.querySelectorAll(".status-chip").some((node) => node.textContent === "修复成功" || node.textContent === "业务已恢复"), false);

ui.monitoring.receipts.delete(incident.id);
Object.assign(ui.historyData.detail("incidents", incident.id), { value: newerIncident, stale: true });
const staleAck = ui.monitoring.acknowledgement(newerIncident, ui.historyData.detail("incidents", incident.id));
assert.equal(staleAck.querySelector("button").disabled, true);
await staleAck.querySelector("button").listeners.click();
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, uncertainPosts, "stale incident detail authorized acknowledgement");

const successfulIncident = { ...incident, id: "incident-success", condition: "active" };
Object.assign(ui.historyData.detail("incidents", successfulIncident.id), { value: successfulIncident, stale: false });
location.hash = `#/monitoring/incidents/${successfulIncident.id}`;
const successfulAck = ui.monitoring.acknowledgement(successfulIncident, ui.historyData.detail("incidents", successfulIncident.id));
const successfulStart = uiCalls.length;
uiResponse = () => json({ ...successfulIncident, status: "acknowledged", revision: 2, allowed_actions: [], acknowledgement: { actor: "server-actor", note: "", at_ms: 4000 } });
await successfulAck.querySelector("button").listeners.click();
assert.deepEqual(JSON.parse(uiCalls[successfulStart].options.body), { revision: 1, note: "" }, "optional note was not supported or client supplied an actor");
assert.equal(ui.monitoring.receipts.get(successfulIncident.id).status, "acknowledged");
assert.equal(ui.historyData.detail("incidents", successfulIncident.id).value.condition, "active");
assert.equal(ui.historyData.detail("incidents", successfulIncident.id).value.acknowledgement.actor, "server-actor");
const successfulPostCount = uiCalls.filter((call) => call.options.method === "POST").length;
await successfulAck.querySelector("button").listeners.click();
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, successfulPostCount, "acknowledged incident accepted a duplicate click");

location.hash = "#/monitoring";
Object.assign(ui.historyData.page("incidents"), { items: [incident], total: 2, next_cursor: incident.id, loaded: true });
uiResponse = () => json({ items: [{ ...resolvedIncident, id: "older-incident" }], total: 2, next_cursor: null });
const incidentList = ui.monitoring.incidentList();
await incidentList.querySelectorAll("button").find((button) => button.textContent === "下一页").listeners.click();
assert.match(incidentList.textContent, /older-incident/);
const incidentQuery = new URL(uiCalls.at(-1).url).searchParams;
assert.equal(incidentQuery.get("status"), "active");
assert.equal(incidentQuery.get("cursor"), incident.id);
assert.equal(incidentQuery.get("limit"), "25");
ui.applySnapshot(monitoringSnapshot);
assert.equal(ui.historyData.page("incidents").items[0].id, "older-incident", "bootstrap replaced an older incident history page");

// Plugin monitoring stays inside the official shell: the bootstrap supplies a
// plugin-only directory, while a fixed GET returns a bounded declarative view.
const pluginId = "plugin-live";
const pluginSummary = { id: pluginId, registration: "registered", registration_error: null, view_registration: "registered", view_error: null, monitor_count: 1, target_count: 1, discovery_count: 1 };
const pluginMonitor = { ...monitor, extension_id: pluginId, owner_plugin_id: pluginId, view_role: "snapshot", last_value: { text_value: malicious, flag_value: true, duration_ms: 1250 } };
const pluginDiscovery = { id: "plugin-inventory", extension_id: pluginId, owner_plugin_id: pluginId, running: true, complete: true, known_targets: 1, present_targets: 1, last_received_at_ms: 2000, last_error: null };
const pluginSnapshot = {
  ...monitoringSnapshot,
  runtime: { ...snapshot.runtime, permissions: ["monitor.read", "incident.read", "incident.acknowledge", "extension.read"] },
  extension_statuses: [{ id: "node-only", kind: "node", available: true, error: null }],
  monitoring: { configured: true, monitors: [pluginMonitor], discoveries: [pluginDiscovery], plugins: [pluginSummary], runtime_error: null },
};
ui.applySnapshot(pluginSnapshot);
location.hash = "#/monitoring";
ui.renderApp({ focus: false });
assert.match(app.textContent, /插件监控/);
assert.match(app.textContent, /plugin-live/);
assert.equal(app.textContent.includes("node-only"), false, "a node was listed as a plugin monitoring page");
const pluginLink = app.querySelectorAll("a").find((anchor) => anchor.getAttribute("href") === "#/monitoring/plugins/plugin-live");
assert.ok(pluginLink, "plugin directory did not expose the host-owned plugin route");

const pluginView = {
  schema_version: 1,
  title: `Example ${malicious}`,
  summary: "插件声明的运行细节",
  sections: [
    {
      id: "runtime",
      title: `运行上下文 ${malicious}`,
      description: malicious,
      monitor_role: "snapshot",
      fields: [
        { id: "text_value", label: "文本值", source: "last_value", pointer: "/text_value", format: "text" },
        { id: "flag_value", label: "标志值", source: "last_value", pointer: "/flag_value", format: "boolean" },
        { id: "duration", label: "持续时间", source: "last_value", pointer: "/duration_ms", format: "duration_ms" },
      ],
    },
    {
      id: "terminal",
      title: "终止状态",
      monitor_role: "terminal",
      fields: [{ id: "event", label: "最近事件", source: "last_value", pointer: "/event", format: "text" }],
    },
  ],
};
const pluginPage = { plugin: pluginSummary, view_status: "ready", view_error: null, view: pluginView, monitors: [pluginMonitor], discoveries: [pluginDiscovery], read_at: 5000, auto_retry: false };
const pluginPostCount = uiCalls.filter((call) => call.options.method === "POST").length;
const pluginGetStart = uiCalls.length;
uiResponse = () => json(pluginPage);
navigate("#/monitoring/plugins/plugin-live");
await new Promise((resolve) => setImmediate(resolve));
assert.match(announcer.textContent, /插件监控/);
assert.equal(uiCalls.slice(pluginGetStart).filter((call) => new URL(call.url).pathname === "/api/v1/monitoring/plugins/plugin-live").length, 1, "first plugin visit did not issue exactly one read");
const pluginRequest = uiCalls.slice(pluginGetStart).find((call) => new URL(call.url).pathname === "/api/v1/monitoring/plugins/plugin-live");
assert.equal(pluginRequest.options.method, "GET");
assert.equal(pluginRequest.options.body, undefined);
assert.match(app.textContent, /宿主权威监控/);
assert.match(app.textContent, /健康状态未知/);
assert.match(app.textContent, /插件专属只读视图/);
assert.match(app.textContent, /文本值/);
assert.match(app.textContent, /未提供/);
assert.equal(app.textContent.includes(malicious), true, "plugin text was altered or dropped");
assert.equal(app.querySelectorAll("img").length, 0);
assert.equal(app.querySelectorAll("script").length, 0);
assert.equal(app.querySelectorAll("iframe").length, 0);
assert.equal(app.querySelectorAll("form").length, 0, "plugin declaration created an action surface");
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, pluginPostCount, "plugin monitoring dispatched a POST");

// Manual monitor refresh uses the same validated plugin catalog path as a
// bootstrap: registration changes invalidate but never auto-retry the view.
const pluginReadsBeforeRegistrationChange = uiCalls.filter((call) => new URL(call.url).pathname === "/api/v1/monitoring/plugins/plugin-live").length;
const changedPluginSummary = { ...pluginSummary, view_error: "registration changed" };
uiResponse = () => json({ configured: true, items: [pluginMonitor], discoveries: [pluginDiscovery], plugins: [changedPluginSummary], runtime_error: null, running: true, counts: pluginSnapshot.monitoring.counts });
await ui.monitoring.refreshMonitors();
await new Promise((resolve) => setImmediate(resolve));
ui.renderApp({ focus: false });
assert.equal(uiCalls.filter((call) => new URL(call.url).pathname === "/api/v1/monitoring/plugins/plugin-live").length, pluginReadsBeforeRegistrationChange, "registration change automatically retried the plugin view");
assert.equal(ui.monitoring.pluginPages.get(pluginId).stale, true, "manual monitor refresh bypassed plugin cache invalidation");

// A registration fingerprint change may replace an in-flight read.  The
// aborted request must not clear the replacement request's ownership state.
let pluginRaceRequest = 0;
let resolveLatestPluginRead;
uiResponse = (_url, { signal }) => {
  pluginRaceRequest += 1;
  if (pluginRaceRequest === 1) {
    return new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true }));
  }
  return new Promise((resolve, reject) => {
    resolveLatestPluginRead = () => resolve(json({ ...pluginPage, read_at: 5001 }));
    signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")), { once: true });
  });
};
const replacedPluginRead = ui.monitoring.readPlugin(pluginId, true);
const replacedController = ui.monitoring.pluginPages.get(pluginId).controller;
ui.applySnapshot(pluginSnapshot);
const latestPluginRead = ui.monitoring.readPlugin(pluginId, true);
const pluginRacePage = ui.monitoring.pluginPages.get(pluginId);
const latestController = pluginRacePage.controller;
assert.notEqual(latestController, replacedController, "fingerprint change did not replace the plugin request controller");
await replacedPluginRead;
assert.equal(pluginRacePage.controller, latestController, "aborted plugin request cleared the replacement controller");
assert.equal(pluginRacePage.loading, true, "aborted plugin request cleared the replacement loading state");
resolveLatestPluginRead();
await latestPluginRead;
assert.equal(pluginRacePage.controller, null);
assert.equal(pluginRacePage.loading, false);
assert.equal(pluginRacePage.value.read_at, 5001);

// Duplicate target/role matches are explicit ambiguity, never an inferred value.
const duplicateMonitor = { ...pluginMonitor, id: "monitor-live-duplicate", last_value: { text_value: "other", flag_value: false, duration_ms: 10 } };
uiResponse = () => json({ ...pluginPage, monitors: [pluginMonitor, duplicateMonitor], read_at: 5001 });
await ui.monitoring.readPlugin(pluginId, true);
ui.renderApp({ focus: false });
assert.match(app.textContent, /歧义（匹配 2 项监控）/);

// An invalid declaration is rejected as a whole while the last successful
// declaration and host-owned fallback remain visible and explicitly stale.
uiResponse = () => json({ ...pluginPage, view: { ...pluginView, html: "<b>not allowed</b>" }, read_at: 5002 });
await ui.monitoring.readPlugin(pluginId, true);
ui.renderApp({ focus: false });
assert.match(app.textContent, /刷新失败，保留上次成功内容/);
assert.match(app.textContent, /正在显示陈旧的插件专属内容/);
assert.match(app.textContent, /宿主权威监控/);

// A current unavailable response also keeps the last successful declaration
// stale; registration failure never removes the generic host monitoring page.
const offlinePlugin = { ...pluginSummary, registration: "unavailable", registration_error: "plugin process offline", view_registration: "unavailable", view_error: "view route offline" };
uiResponse = () => json({ plugin: offlinePlugin, view_status: "unavailable", view_error: "plugin process offline", view: null, monitors: [pluginMonitor], discoveries: [pluginDiscovery], read_at: 5003, auto_retry: false });
await ui.monitoring.readPlugin(pluginId, true);
ui.applySnapshot({ ...pluginSnapshot, monitoring: { ...pluginSnapshot.monitoring, plugins: [offlinePlugin] } });
await new Promise((resolve) => setImmediate(resolve));
ui.renderApp({ focus: false });
assert.match(app.textContent, /插件注册不可用/);
assert.match(app.textContent, /专属视图当前不可用/);
assert.match(app.textContent, /宿主权威监控/);
assert.equal(uiCalls.filter((call) => call.options.method === "POST").length, pluginPostCount);

// A current authoritative permission denial must remove previously cached
// plugin-provided content instead of retaining it as a stale disclosure.
ui.applySnapshot(pluginSnapshot);
uiResponse = () => json({ ...pluginPage, view_status: "permission_denied", view_error: "extension.read permission is required", view: null, read_at: 5004 });
await ui.monitoring.readPlugin(pluginId, true);
ui.renderApp({ focus: false });
assert.equal(ui.monitoring.pluginPages.get(pluginId).value, null, "permission denial retained plugin-provided content");
assert.match(app.textContent, /无权读取专属视图/);
assert.equal(app.textContent.includes("插件专属只读视图"), false);
assert.equal(app.textContent.includes("正在显示陈旧的插件专属内容"), false);

uiResponse = () => json({ ...pluginPage, read_at: 5005 });
await ui.monitoring.readPlugin(pluginId, true);
assert.ok(ui.monitoring.pluginPages.get(pluginId).value);
uiResponse = () => json({ ...pluginPage, view_status: "invalid_registration", view_error: "descriptor is no longer allowlisted", view: null, read_at: 5006 });
await ui.monitoring.readPlugin(pluginId, true);
assert.equal(ui.monitoring.pluginPages.get(pluginId).value, null, "invalid registration retained formerly trusted plugin content");
uiResponse = () => json({ ...pluginPage, read_at: 5007 });
await ui.monitoring.readPlugin(pluginId, true);
assert.ok(ui.monitoring.pluginPages.get(pluginId).value);
uiResponse = () => json({ error: { code: "forbidden", message: "extension.read was revoked" } }, 403);
await ui.monitoring.readPlugin(pluginId, true);
assert.equal(ui.monitoring.pluginPages.get(pluginId).value, null, "HTTP permission denial retained plugin-provided content");
assert.equal(ui.monitoring.pluginPages.get(pluginId).current, null, "HTTP permission denial retained a successful runtime status");

ui.applySnapshot({ ...monitoringSnapshot, runtime: { ...snapshot.runtime, permissions: [] }, monitoring: null });
assert.equal(ui.historyData.details.has(`incidents:${incident.id}`), false, "permission loss retained incident evidence");
assert.equal(ui.monitoring.drafts.size, 0);
assert.equal(ui.monitoring.pluginPages.size, 0, "permission loss retained plugin monitoring content");
// A real bootstrap 401 clears identity-scoped evidence and returns to auth.
ui.historyData.detail("approvals", "private-expiring").value = { expected: "private evidence" };
uiResponse = () => json({ error: { code: "unauthorized", message: "expired" } }, 401);
document.hidden = false;
ui.startPolling();
await new Promise((resolve) => setImmediate(resolve));
document.hidden = true;
assert.equal(ui.connection.hasSnapshot, false);
assert.equal(ui.api.configured, false);
assert.equal(ui.historyData.details.size, 0);
assert.equal(app.querySelectorAll(".sidebar").length, 0);
assert.equal(location.hash, "#/auth");
ui.monitoring.pluginPages.set("private-plugin", { controller: null, value: pluginPage });
ui.selectDemo(true);
assert.equal(ui.monitoring.pluginPages.size, 0, "entering demo retained plugin monitoring content");
navigate("#/overview");
const demoMonitoring = ui.monitoring.render({ section: "monitoring" });
assert.match(demoMonitoring.textContent, /演示模式没有真实监控记录/);
assert.equal(demoMonitoring.textContent.includes(incident.id), false);
assert.equal(demoMonitoring.querySelectorAll("button").length, 0);
ui.selectDemo(false);
assert.equal(ui.api.configured, false);
assert.equal(ui.historyData.details.size, 0);
assert.equal(ui.monitoring.receipts.size, 0);
for (const timer of timers) clearTimeout(timer);
ui.api.disconnect();
await import("./ui_diff.mjs");
await import("./project_logs.mjs");
console.log("UI client and renderer behaviour passed: connection, bounds, pagination, detail caching, monitoring dimensions, plugin declarative views and fallback, incident acknowledgement, stale revisions, drafts, no POST retries, Unknown, and safe text.");
