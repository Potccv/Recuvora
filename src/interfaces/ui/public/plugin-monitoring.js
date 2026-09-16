// Declarative plugin monitoring views. Plugin data is always rendered as bounded plain text.
import { ApiError } from "./api.js";
import { el, formatDate, section, table, utf8Bytes } from "./dom.js";

const MAX_VIEW_BYTES = 32 * 1024;
const MAX_SECTIONS = 8;
const MAX_FIELDS = 64;
const MAX_CELL_TEXT = 4096;
const idPattern = /^[A-Za-z0-9._-]{1,128}$/;
const sources = new Set(["monitor", "last_value"]);
const formats = new Set(["text", "number", "boolean", "timestamp_ms", "duration_ms", "json"]);

function invalid(message = "插件监控视图结构不兼容。") {
  throw new ApiError("invalid_response", message);
}

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value, allowed, required) {
  if (!object(value)) invalid();
  const keys = Object.keys(value);
  if (keys.some((key) => !allowed.includes(key)) || required.some((key) => !keys.includes(key))) invalid();
}

function boundedString(value, maxBytes, { optional = false, allowEmpty = false } = {}) {
  if (optional && (value === null || value === undefined)) return null;
  if (typeof value !== "string" || value.includes("\0") || utf8Bytes(value) > maxBytes || (!allowEmpty && !value.trim())) invalid();
  return value;
}

export function validPluginId(value) {
  return typeof value === "string" && idPattern.test(value);
}

function validPointer(value) {
  if (typeof value !== "string" || value.includes("\0") || utf8Bytes(value) > 256) return false;
  if (value === "") return true;
  if (!value.startsWith("/")) return false;
  const parts = value.slice(1).split("/");
  return parts.length <= 16 && parts.every((part) => !/~(?:[^01]|$)/.test(part));
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function optionalError(value) {
  if (value === null || value === undefined) return null;
  return boundedString(value, 4096, { allowEmpty: true });
}

export function validatePluginSummary(value, expectedId = null) {
  exactKeys(value,
    ["id", "registration", "registration_error", "view_registration", "view_error", "monitor_count", "target_count", "discovery_count"],
    ["id", "registration", "registration_error", "view_registration", "view_error", "monitor_count", "target_count", "discovery_count"]);
  const id = boundedString(value.id, 128);
  if (!validPluginId(id) || (expectedId !== null && id !== expectedId)) invalid("插件监控响应与请求实例不匹配。");
  if (typeof value.registration !== "string" || typeof value.view_registration !== "string") invalid();
  for (const count of [value.monitor_count, value.target_count, value.discovery_count]) if (!nonNegativeInteger(count)) invalid();
  return {
    id,
    registration: boundedString(value.registration, 64),
    registration_error: optionalError(value.registration_error),
    view_registration: boundedString(value.view_registration, 64),
    view_error: optionalError(value.view_error),
    monitor_count: value.monitor_count,
    target_count: value.target_count,
    discovery_count: value.discovery_count,
  };
}

export function validatePluginView(value) {
  let encoded;
  try { encoded = JSON.stringify(value); } catch { invalid(); }
  if (typeof encoded !== "string" || utf8Bytes(encoded) > MAX_VIEW_BYTES) invalid("插件监控视图超过 32 KiB 前端复核上限。");
  exactKeys(value, ["schema_version", "title", "summary", "sections"], ["schema_version", "title", "sections"]);
  if (value.schema_version !== 1 || !Array.isArray(value.sections) || value.sections.length > MAX_SECTIONS) invalid();
  const sectionIds = new Set();
  const fieldIds = new Set();
  let fieldCount = 0;
  const sections = value.sections.map((entry) => {
    exactKeys(entry, ["id", "title", "description", "monitor_role", "fields"], ["id", "title", "monitor_role", "fields"]);
    const id = boundedString(entry.id, 128);
    const monitorRole = boundedString(entry.monitor_role, 128);
    if (!validPluginId(id) || !validPluginId(monitorRole) || sectionIds.has(id) || !Array.isArray(entry.fields) || !entry.fields.length) invalid();
    sectionIds.add(id);
    fieldCount += entry.fields.length;
    if (fieldCount > MAX_FIELDS) invalid();
    const fields = entry.fields.map((field) => {
      exactKeys(field, ["id", "label", "source", "pointer", "format", "unit", "empty"], ["id", "label", "source", "pointer", "format"]);
      const fieldId = boundedString(field.id, 128);
      if (!validPluginId(fieldId) || fieldIds.has(fieldId) || !sources.has(field.source) || !formats.has(field.format) || !validPointer(field.pointer)) invalid();
      fieldIds.add(fieldId);
      return {
        id: fieldId,
        label: boundedString(field.label, 128),
        source: field.source,
        pointer: field.pointer,
        format: field.format,
        unit: boundedString(field.unit, 32, { optional: true, allowEmpty: true }),
        empty: boundedString(field.empty, 128, { optional: true, allowEmpty: true }),
      };
    });
    return {
      id,
      title: boundedString(entry.title, 128),
      description: boundedString(entry.description, 1024, { optional: true, allowEmpty: true }),
      monitor_role: monitorRole,
      fields,
    };
  });
  return {
    schema_version: 1,
    title: boundedString(value.title, 128),
    summary: boundedString(value.summary, 1024, { optional: true, allowEmpty: true }),
    sections,
  };
}

function validateOwnedItems(items, expectedId, limit, kind) {
  if (!Array.isArray(items) || items.length > limit) invalid(`插件监控${kind}集合结构不兼容。`);
  return items.map((item) => {
    if (!object(item) || item.owner_plugin_id !== expectedId || !validPluginId(item.id)) invalid(`插件监控${kind}归属不兼容。`);
    return item;
  });
}

export function validatePluginPage(value, expectedId) {
  exactKeys(value,
    ["plugin", "view_status", "view_error", "view", "monitors", "discoveries", "read_at", "auto_retry"],
    ["plugin", "view_status", "view_error", "view", "monitors", "discoveries", "read_at", "auto_retry"]);
  if (typeof value.view_status !== "string" || value.view_status.includes("\0") || utf8Bytes(value.view_status) > 64 || value.auto_retry !== false || !nonNegativeInteger(value.read_at)) invalid();
  const view = value.view === null ? null : validatePluginView(value.view);
  if (value.view_status === "ready" && view === null) invalid();
  return {
    plugin: validatePluginSummary(value.plugin, expectedId),
    view_status: value.view_status,
    view_error: optionalError(value.view_error),
    view,
    monitors: validateOwnedItems(value.monitors, expectedId, 64, "监控"),
    discoveries: validateOwnedItems(value.discoveries, expectedId, 8, "发现"),
    read_at: value.read_at,
    auto_retry: false,
  };
}

function pointerValue(root, pointer) {
  if (pointer === "") return root;
  let value = root;
  for (const encoded of pointer.slice(1).split("/")) {
    const key = encoded.replace(/~1/g, "/").replace(/~0/g, "~");
    if ((value === null || typeof value !== "object") || !Object.prototype.hasOwnProperty.call(value, key)) return undefined;
    value = value[key];
  }
  return value;
}

function boundedCell(value) {
  const text = String(value);
  return text.length > MAX_CELL_TEXT ? `${text.slice(0, MAX_CELL_TEXT)}…[已截断]` : text;
}

function safeJsonNumbers(value, depth = 0) {
  if (depth > 32) return false;
  if (typeof value === "number") return Number.isFinite(value) && Math.abs(value) <= Number.MAX_SAFE_INTEGER;
  if (Array.isArray(value)) return value.every((entry) => safeJsonNumbers(entry, depth + 1));
  if (object(value)) return Object.values(value).every((entry) => safeJsonNumbers(entry, depth + 1));
  return true;
}

function formatField(field, monitor) {
  const root = field.source === "monitor" ? monitor : monitor.last_value;
  const value = pointerValue(root, field.pointer);
  const empty = field.empty ?? "未提供";
  if (value === undefined || value === null) return empty;
  switch (field.format) {
    case "text": return typeof value === "string" ? boundedCell(value) : empty;
    case "number": return typeof value === "number" && Number.isFinite(value) && Math.abs(value) <= Number.MAX_SAFE_INTEGER ? boundedCell(`${value}${field.unit ? ` ${field.unit}` : ""}`) : empty;
    case "boolean": return typeof value === "boolean" ? (value ? "是" : "否") : empty;
    case "timestamp_ms": return Number.isSafeInteger(value) && value >= 0 ? formatDate(value, true) : empty;
    case "duration_ms": return Number.isSafeInteger(value) && value >= 0 ? boundedCell(`${value} ${field.unit || "ms"}`) : empty;
    case "json": {
      if (!safeJsonNumbers(value)) return empty;
      let text;
      try { text = JSON.stringify(value); } catch { return empty; }
      return text === undefined ? empty : boundedCell(text);
    }
    default: return empty;
  }
}

export function renderPluginView(view, monitors) {
  const targets = [...new Set(monitors.map((monitor) => monitor.target_id).filter((target) => typeof target === "string" && target))].sort();
  const rowsFor = (viewSection) => {
    const shownTargets = targets.length ? targets : [null];
    return shownTargets.map((target) => {
      const matches = target === null ? [] : monitors.filter((monitor) => monitor.target_id === target && monitor.view_role === viewSection.monitor_role);
      const cells = viewSection.fields.map((field) => matches.length === 1 ? formatField(field, matches[0]) : matches.length === 0 ? (field.empty ?? "未提供") : `歧义（匹配 ${matches.length} 项监控）`);
      return [target ?? "未提供", ...cells];
    });
  };
  return section("插件专属只读视图", view.summary ?? "插件声明的专属信息仅供查看，不是宿主健康、授权、修复或业务恢复事实。", el("div", { className: "grid" },
    view.sections.length ? view.sections.map((viewSection) => section(
      viewSection.title,
      [viewSection.description, `监控角色：${viewSection.monitor_role}`].filter(Boolean).join(" · "),
      table(["目标", ...viewSection.fields.map((field) => field.label)], rowsFor(viewSection), `${viewSection.title}（插件只读声明）`),
    )) : el("p", { className: "muted" }, "插件声明中没有专属展示区块。"),
  ));
}

export const pluginRegistrationStates = Object.freeze({
  registered: { label: "插件已注册", tone: "success" },
  unavailable: { label: "插件注册不可用", tone: "warning" },
  disabled: { label: "插件已停用", tone: "neutral" },
});

export const viewRegistrationStates = Object.freeze({
  registered: { label: "专属视图已登记", tone: "success" },
  not_registered: { label: "未登记专属视图", tone: "neutral" },
  invalid: { label: "专属视图登记无效", tone: "danger" },
  unavailable: { label: "专属视图不可用", tone: "warning" },
});

export const viewRuntimeStates = Object.freeze({
  ready: { label: "专属视图已读取", tone: "success" },
  not_registered: { label: "未登记专属视图", tone: "neutral" },
  invalid_registration: { label: "专属视图登记无效", tone: "danger" },
  permission_denied: { label: "无权读取专属视图", tone: "warning" },
  unavailable: { label: "专属视图当前不可用", tone: "warning" },
  invalid_response: { label: "专属视图响应无效", tone: "danger" },
});

export function knownState(catalog, value) {
  return catalog[value] ?? { label: "状态未知", tone: "unknown" };
}
