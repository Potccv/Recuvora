// Safe, stateless presentation primitives shared by the official pages.
import { copyPlainText } from "./shell.js";

const iconPaths = Object.freeze({
  overview: "M4 13h6V4H4v9Zm10 7h6v-9h-6v9ZM4 20h6v-3H4v3Zm10-13h6V4h-6v3Z",
  repair: "m14.7 6.3 3-3a2.8 2.8 0 0 1-3.9 3.9l-7.9 7.9-3 .8.8-3 7.9-7.9a2.8 2.8 0 0 1 3.1-3.7Z",
  approval: "M12 3 5 6v5c0 4.7 2.8 8.2 7 10 4.2-1.8 7-5.3 7-10V6l-7-3Zm-3 9 2 2 4-4",
  harness: "M8 3h8v4H8V3ZM5 17h4v4H5v-4Zm10 0h4v4h-4v-4Zm-3-10v5m-5 5v-2h10v2",
  simulation: "M9 3h6m-5 0v5l-5.3 9.2A2.5 2.5 0 0 0 6.9 21h10.2a2.5 2.5 0 0 0 2.2-3.8L14 8V3M7.5 16h9",
  capability: "M4 4h6v6H4V4Zm10 0h6v6h-6V4ZM4 14h6v6H4v-6Zm10 0h6v6h-6v-6Z",
  settings: "M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Zm7.4-3.5a7.5 7.5 0 0 0-.1-1l2-1.6-2-3.4-2.5 1a8 8 0 0 0-1.7-1L14.7 3h-4l-.4 3a8 8 0 0 0-1.7 1L6.1 6l-2 3.4 2 1.6a7.5 7.5 0 0 0 0 2l-2 1.6 2 3.4 2.5-1a8 8 0 0 0 1.7 1l.4 3h4l.4-3a8 8 0 0 0 1.7-1l2.5 1 2-3.4-2-1.6c.1-.3.1-.7.1-1Z",
  info: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Zm0-11v6m0-10h.01",
  menu: "M4 7h16M4 12h16M4 17h16",
  warning: "M12 3 2.8 20h18.4L12 3Zm0 6v5m0 3h.01",
  check: "m5 12 4 4L19 6",
  close: "M6 6l12 12M18 6 6 18",
  clock: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Zm0-15v5l3 2",
  arrow: "M5 12h14m-5-5 5 5-5 5",
  chevron: "m9 18 6-6-6-6",
  search: "m21 21-4.4-4.4m2.4-5.1a7.5 7.5 0 1 1-15 0 7.5 7.5 0 0 1 15 0Z",
  plus: "M12 5v14M5 12h14",
  lock: "M7 11V8a5 5 0 0 1 10 0v3m-11 0h12v10H6V11Z",
  copy: "M8 8h11v13H8V8Zm-3 8H3V3h11v2",
  file: "M6 2h8l4 4v16H6V2Zm8 0v5h5",
  target: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Zm0-4a6 6 0 1 0 0-12 6 6 0 0 0 0 12Zm0-4a2 2 0 1 0 0-4 2 2 0 0 0 0 4Z",
  activity: "M3 12h4l2-7 4 14 2-7h6",
  terminal: "m5 7 4 5-4 5m7 0h7",
  folder: "M3 6h7l2 2h9v11H3V6Z",
  external: "M14 4h6v6m0-6-9 9M10 6H4v14h14v-6",
  shield: "M12 3 5 6v5c0 4.7 2.8 8.2 7 10 4.2-1.8 7-5.3 7-10V6l-7-3Z",
  eye: "M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Zm10 3a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z",
  play: "m8 5 11 7-11 7V5Z",
  stop: "M6 6h12v12H6V6Z",
  database: "M4 6c0-2 3.6-3 8-3s8 1 8 3-3.6 3-8 3-8-1-8-3Zm0 0v6c0 2 3.6 3 8 3s8-1 8-3V6m-16 6v6c0 2 3.6 3 8 3s8-1 8-3v-6",
});

function appendChild(parent, child) {
  if (child === null || child === undefined || child === false) {
    return;
  }
  if (Array.isArray(child)) {
    child.forEach((value) => appendChild(parent, value));
    return;
  }
  if (child instanceof Node) {
    parent.append(child);
    return;
  }
  parent.append(document.createTextNode(String(child)));
}

function el(tag, properties = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(properties)) {
    if (value === null || value === undefined || value === false) {
      continue;
    }
    if (key === "className") {
      element.className = value;
    } else if (key === "dataset") {
      for (const [dataKey, dataValue] of Object.entries(value)) {
        element.dataset[dataKey] = String(dataValue);
      }
    } else if (key === "on") {
      element.__recuvoraListeners ??= {};
      for (const [eventName, listener] of Object.entries(value)) {
        element.addEventListener(eventName, listener);
        element.__recuvoraListeners[eventName] = listener;
      }
    } else if (key === "htmlFor") {
      element.htmlFor = value;
    } else if (key === "disabled") {
      element.disabled = Boolean(value);
    } else if (key === "checked") {
      element.checked = Boolean(value);
    } else if (key === "value") {
      element.value = value;
    } else if (key === "tabIndex") {
      element.tabIndex = value;
    } else {
      element.setAttribute(key, String(value));
    }
  }
  children.forEach((child) => appendChild(element, child));
  return element;
}

function icon(name, size = 18) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", iconPaths[name] ?? iconPaths.info);
  svg.append(path);
  return svg;
}

function safeSegment(value) {
  return encodeURIComponent(String(value));
}

function formatDate(value, includeDate = false) {
  if (value === null || value === undefined || value === "") return "未提供";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return String(value);
  }
  return new Intl.DateTimeFormat("zh-CN", {
    month: includeDate ? "2-digit" : undefined,
    day: includeDate ? "2-digit" : undefined,
    hour: "2-digit",
    minute: "2-digit",
    second: includeDate ? undefined : "2-digit",
    hour12: false,
  }).format(date);
}

function utf8Bytes(value) {
  return new TextEncoder().encode(value).length;
}

function bindUtf8Limit(control, counter, maxBytes) {
  const update = () => {
    const bytes = utf8Bytes(control.value);
    const hasValue = control.value.length > 0;
    const valid = control.value.trim().length > 0
      && !control.value.includes("\0")
      && bytes <= maxBytes;
    counter.textContent = `${bytes} / ${maxBytes} 字节`;
    counter.classList.toggle("byte-counter-invalid", hasValue && !valid);
    control.setAttribute("aria-invalid", String(hasValue && !valid));
    control.setCustomValidity(valid ? "" : `请输入 1–${maxBytes} 个 UTF-8 字节，且不得包含 NUL。`);
  };
  control.addEventListener("input", update);
  update();
}

function statusChip(meta, options = {}) {
  meta ??= { label: "未提供状态", tone: "neutral" };
  const label = options.prefix ? `${options.prefix}${meta.label}` : meta.label;
  return el(
    "span",
    { className: `status-chip tone-${meta.tone}`, title: options.title ?? meta.label },
    el("span", { className: "status-dot", "aria-hidden": "true" }),
    label,
  );
}

function tag(label, options = {}) {
  const classes = ["tag"];
  if (options.mono) classes.push("tag-mono");
  if (options.demo) classes.push("demo-badge");
  return el("span", { className: classes.join(" ") }, label);
}

function linkButton(label, href, options = {}) {
  const classes = ["button", options.variant ? `button-${options.variant}` : ""]
    .filter(Boolean)
    .join(" ");
  return el(
    "a",
    { className: classes, href },
    options.iconName ? icon(options.iconName, 15) : null,
    label,
  );
}

function actionButton(label, options = {}) {
  const classes = ["button", options.variant ? `button-${options.variant}` : ""]
    .filter(Boolean)
    .join(" ");
  return el(
    "button",
    {
      className: classes,
      type: options.type ?? "button",
      disabled: options.disabled,
      title: options.title,
      "aria-label": options.ariaLabel,
      on: options.onClick ? { click: options.onClick } : undefined,
    },
    options.iconName ? icon(options.iconName, 15) : null,
    label,
  );
}

function disabledAction(label, options = {}) {
  return actionButton(label, {
    ...options,
    iconName: options.iconName ?? "lock",
    disabled: true,
    title: options.title ?? "UI 预览未连接可信运行时，操作不可提交",
  });
}

function showToast(message) {
  const region = document.querySelector(".toast-region");
  if (!region) return;
  const toast = el("div", { className: "toast", role: "status" }, message);
  region.append(toast);
  setTimeout(() => toast.remove(), 2400);
}

function copyable(value, label = "复制") {
  if (value === null || value === undefined || value === "") return el("span", { className: "muted" }, "未提供");
  return el(
    "span",
    { className: "copy-row" },
    el("span", { className: "id-value" }, value),
    el(
      "button",
      {
        className: "copy-button",
        type: "button",
        dataset: { refreshEvents: "true" },
        title: label,
        "aria-label": `${label}：${value}`,
        on: {
          click: async () => {
            const copied = await copyPlainText(value);
            showToast(copied ? "已复制到剪贴板" : "当前环境无法访问剪贴板");
          },
        },
      },
      icon("copy", 13),
    ),
  );
}

function detailList(rows) {
  return el(
    "dl",
    { className: "detail-list" },
    rows.map(([label, value]) =>
      el("div", { className: "detail-row" }, el("dt", {}, label), el("dd", {}, value)),
    ),
  );
}

function card(title, subtitle, body, options = {}) {
  return el(
    "section",
    { className: `card ${options.className ?? ""}`.trim() },
    title
      ? el(
          "div",
          { className: "card-header" },
          el(
            "div",
            {},
            el("h2", { className: "card-title" }, title),
            subtitle ? el("div", { className: "card-subtitle" }, subtitle) : null,
          ),
          options.headerAction ?? null,
        )
      : null,
    el("div", { className: "card-body" }, body),
    options.footer ? el("div", { className: "card-footer" }, options.footer) : null,
  );
}

function callout(kind, title, copy, options = {}) {
  return el(
    "div",
    { className: `callout callout-${kind}`, role: options.role ?? "note" },
    el("span", { className: "callout-icon" }, icon(options.iconName ?? "warning", 18)),
    el("div", {}, el("strong", {}, title), el("p", {}, copy)),
  );
}

function pageHeader(eyebrow, title, description, actions = []) {
  const heading = el("h1", { tabIndex: -1 }, title);
  return el(
    "header",
    { className: "page-header" },
    el(
      "div",
      { className: "page-heading" },
      el("div", { className: "eyebrow" }, eyebrow),
      heading,
      el("p", { className: "page-description" }, description),
    ),
    actions.length ? el("div", { className: "page-actions" }, actions) : null,
  );
}

function sectionHeader(title, description, action = null) {
  return el(
    "div",
    { className: "section-header" },
    el(
      "div",
      { className: "section-title-row" },
      el("h2", {}, title),
      description ? el("p", { className: "section-description" }, description) : null,
    ),
    action,
  );
}

function section(title, description, content, action = null) {
  return el(
    "section",
    { className: "section" },
    sectionHeader(title, description, action),
    content,
  );
}

function metricCard(label, value, detail, iconName, tone = "primary", href = null) {
  const safeTone = ["primary", "warning", "unknown", "success"].includes(tone) ? tone : "primary";
  const content = el(
    "div",
    {
      className: `card metric-card card-interactive metric-tone-${safeTone}`,
    },
    el(
      "div",
      { className: "metric-top" },
      el("span", { className: "metric-label" }, label),
      el("span", { className: "metric-icon" }, icon(iconName, 16)),
    ),
    el("div", { className: "metric-value" }, value),
    el("div", { className: "metric-detail" }, detail),
  );
  return href ? el("a", { href, "aria-label": `${label}：${value}` }, content) : content;
}

function table(headers, rows, caption) {
  return el(
    "div",
    { className: "card table-card" },
    el(
      "div",
      { className: "table-scroll", role: "region", "aria-label": caption, tabIndex: 0 },
      el(
        "table",
        { className: "data-table" },
        el("caption", { className: "sr-only" }, caption),
        el(
          "thead",
          {},
          el("tr", {}, headers.map((header) => el("th", { scope: "col" }, header))),
        ),
        el(
          "tbody",
          {},
          rows.map((cells) => el("tr", {}, cells.map((cell) => el("td", {}, cell)))),
        ),
      ),
    ),
  );
}

function textFrame(label, text, secondary = "不可信纯文本") {
  return el(
    "div",
    { className: "text-frame" },
    el("div", { className: "text-frame-label" }, el("span", {}, label), el("span", {}, secondary)),
    el("pre", { className: "plain-response" }, text),
  );
}

// Preserve line endings and the final newline so presentation cannot hide an
// exact-replacement difference. The dynamic-programming table has a hard cap.
function lineDiff(beforeText, afterText) {
  const lines = (value) => String(value ?? "").match(/[^\n]*\n|[^\n]+$/g) ?? [];
  const before = lines(beforeText);
  const after = lines(afterText);
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) prefix++;
  let suffix = 0;
  while (suffix < before.length - prefix && suffix < after.length - prefix && before[before.length - suffix - 1] === after[after.length - suffix - 1]) suffix++;
  const oldEnd = before.length - suffix;
  const newEnd = after.length - suffix;
  const oldLength = oldEnd - prefix;
  const newLength = newEnd - prefix;
  const rows = [];
  let oldLine = 1;
  let newLine = 1;
  const push = (kind, raw) => {
    const ending = raw.endsWith("\r\n") ? "CRLF" : raw.endsWith("\n") ? "LF" : "none";
    rows.push({ kind, raw, text: ending === "CRLF" ? raw.slice(0, -2) : ending === "LF" ? raw.slice(0, -1) : raw, ending, before: kind === "add" ? null : oldLine++, after: kind === "remove" ? null : newLine++ });
  };
  for (let index = 0; index < prefix; index++) push("same", before[index]);
  const bounded = (oldLength + 1) * (newLength + 1) <= 160000 && oldLength <= 4096 && newLength <= 4096;
  if (bounded && oldLength && newLength) {
    const width = newLength + 1;
    const lengths = new Uint16Array((oldLength + 1) * width);
    for (let oldIndex = oldLength - 1; oldIndex >= 0; oldIndex--) {
      for (let newIndex = newLength - 1; newIndex >= 0; newIndex--) {
        lengths[oldIndex * width + newIndex] = before[prefix + oldIndex] === after[prefix + newIndex]
          ? lengths[(oldIndex + 1) * width + newIndex + 1] + 1
          : Math.max(lengths[(oldIndex + 1) * width + newIndex], lengths[oldIndex * width + newIndex + 1]);
      }
    }
    let oldIndex = 0;
    let newIndex = 0;
    while (oldIndex < oldLength || newIndex < newLength) {
      if (oldIndex < oldLength && newIndex < newLength && before[prefix + oldIndex] === after[prefix + newIndex]) {
        push("same", before[prefix + oldIndex++]); newIndex++;
      } else if (oldIndex < oldLength && (newIndex === newLength || lengths[(oldIndex + 1) * width + newIndex] >= lengths[oldIndex * width + newIndex + 1])) {
        push("remove", before[prefix + oldIndex++]);
      } else {
        push("add", after[prefix + newIndex++]);
      }
    }
  } else {
    for (let index = prefix; index < oldEnd; index++) push("remove", before[index]);
    for (let index = prefix; index < newEnd; index++) push("add", after[index]);
  }
  for (let index = oldEnd; index < before.length; index++) push("same", before[index]);
  return { rows, bounded, removed: rows.filter((row) => row.kind === "remove").length, added: rows.filter((row) => row.kind === "add").length };
}

function diffViewer(approval, demo = false) {
  const diff = lineDiff(approval.expected, approval.replacement);
  const changes = el("div", { className: "line-diff", role: "region", tabIndex: 0, "aria-label": "逐行文件差异，左侧原文行号，右侧替换行号" });
  const visible = new Set();
  diff.rows.forEach((row, index) => {
    if (row.kind !== "same") for (let context = Math.max(0, index - 3); context <= Math.min(diff.rows.length - 1, index + 3); context++) visible.add(context);
  });
  let skipped = 0;
  let rendered = 0;
  const gap = () => {
    if (skipped) changes.append(el("div", { className: "diff-gap" }, `… ${skipped} 行相同内容 …`));
    skipped = 0;
  };
  for (let index = 0; index < diff.rows.length; index++) {
    if (!visible.has(index)) { skipped++; continue; }
    gap();
    if (rendered++ >= 800) {
      changes.append(el("div", { className: "diff-gap" }, "变更较多，逐行视图显示前 800 行。请切换完整文本查看全部内容。"));
      break;
    }
    const row = diff.rows[index];
    changes.append(el("div", { className: `diff-line diff-line-${row.kind}` },
      el("span", { className: "diff-line-number", "aria-label": row.before ? `原文第 ${row.before} 行` : "原文无此行" }, row.before ?? ""),
      el("span", { className: "diff-line-number", "aria-label": row.after ? `替换第 ${row.after} 行` : "替换无此行" }, row.after ?? ""),
      el("span", { className: "diff-line-marker", "aria-label": row.kind === "add" ? "新增" : row.kind === "remove" ? "删除" : "相同" }, row.kind === "add" ? "+" : row.kind === "remove" ? "−" : " "),
      el("code", { className: "diff-line-text" }, row.text || el("span", { className: "diff-empty-line" }, "空行"), row.ending === "none" ? el("span", { className: "diff-ending" }, "行末无换行") : row.ending === "CRLF" ? el("span", { className: "diff-ending" }, "CRLF") : null),
    ));
  }
  gap();
  if (!diff.removed && !diff.added) changes.replaceChildren(el("p", { className: "diff-no-change" }, "原文与提议文本完全相同。"));
  const full = el("div", { className: "diff-grid", hidden: true },
    el("div", { className: "diff-pane diff-pane-before" }, el("div", { className: "diff-pane-header" }, "批准基线 · 完整原文"), el("pre", { className: "untrusted-text", "aria-label": "替换前完整文本", tabIndex: 0 }, approval.expected)),
    el("div", { className: "diff-pane diff-pane-after" }, el("div", { className: "diff-pane-header" }, "提议替换 · 完整文本"), el("pre", { className: "untrusted-text", "aria-label": "替换后完整文本", tabIndex: 0 }, approval.replacement)),
  );
  const switchView = (complete) => {
    changes.hidden = complete;
    full.hidden = !complete;
    changeButton.setAttribute("aria-pressed", String(!complete));
    fullButton.setAttribute("aria-pressed", String(complete));
  };
  const changeButton = el("button", { type: "button", className: "filter-button", "aria-pressed": "true", on: { click: () => switchView(false) } }, "逐行变更");
  const fullButton = el("button", { type: "button", className: "filter-button", "aria-pressed": "false", on: { click: () => switchView(true) } }, "完整文本");
  return el("div", { className: "diff-shell", dataset: { preserveView: "file-diff", requestId: approval.requestId ?? "", revision: approval.revision ?? "" } },
    el("div", { className: "diff-toolbar" }, el("span", { className: "diff-file-path" }, approval.path), el("span", {}, `${demo ? "演示 · " : ""}−${diff.removed} / +${diff.added} 行`)),
    el("div", { className: "diff-view-tabs", role: "group", "aria-label": "文件差异显示方式" }, changeButton, fullButton),
    !diff.bounded ? el("p", { className: "diff-note" }, "此段变更较大，按整段删除与新增展示；完整文本包含全部准确内容。") : null,
    changes, full,
  );
}

function jsonDetails(label, value) {
  return el(
    "details",
    { className: "section" },
    el("summary", { className: "section-link" }, label),
    textFrame("JSON 快照", JSON.stringify(value, null, 2), "纯文本，不执行"),
  );
}

function businessVerificationChip() {
  return statusChip({ label: "业务恢复未验证", tone: "warning" });
}


export { el, icon, safeSegment, formatDate, utf8Bytes, bindUtf8Limit, statusChip, tag, linkButton, actionButton, disabledAction, showToast, copyable, detailList, card, callout, pageHeader, sectionHeader, section, metricCard, table, textFrame, lineDiff, diffViewer, jsonDetails, businessVerificationChip };
