// Observation records are read-only and update their own bounded view, never the app shell.
import { ApiError } from "./api.js";
import { el, pageHeader, linkButton, actionButton, card, callout, formatDate } from "./dom.js";

const MAX_LOGS = 2000;
const MAX_ERRORS = 400;
const MAX_MESSAGE = 16384;
const MAX_TEXT = 2 * 1024 * 1024;
const READ_INTERVAL = 2000;
const terminalCodes = new Set(["logs_unavailable", "not_found", "cursor_invalid", "invalid_response", "response_too_large"]);

function sourceStatusMessage(value) {
  const raw = typeof value === "string" ? value.trim() : "";
  const code = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(raw) ? raw : "unrecognized_source_status";
  return `来源状态代码：${code}。具体含义由已登记的外部来源契约定义。`;
}

export class ProjectLogsConsole {
  constructor({ api, can, isDemo, onAuthError = () => {} }) {
    Object.assign(this, { api, can, isDemo, onAuthError });
    this.clear();
  }
  clear() {
    this.deactivate();
    this.root?.replaceChildren();
    this.id = null;
    this.root = null;
    this.status = null;
    this.notice = null;
    this.toggle = null;
    this.latest = null;
    this.follow = null;
    this.logStream = null;
    this.errorStream = null;
    this.headingTarget = null;
    this.targetId = null;
    this.items = [];
    this.errors = [];
    this.seen = new Set();
    this.cursor = "";
    this.readAt = null;
    this.streamLabel = null;
    this.error = null;
    this.sourceError = null;
    this.coverage = null;
    this.availableStreams = [];
    this.dropped = 0;
    this.failures = 0;
    this.loaded = false;
    this.paused = false;
    this.stopped = false;
  }
  authorized() {
    return !this.isDemo() && ["monitor.read", "logs.read", "extension.read"].every((permission) => this.can(permission));
  }
  render(id) {
    if (this.id !== id) { this.clear(); this.id = id; }
    if (!this.authorized() && (this.loaded || this.items.length || this.errors.length)) this.revokeAccess();
    if (this.isDemo()) return el("div", {}, pageHeader("Observation records", "观测记录", "读取当前监控目标的观测记录。"), callout("info", "演示模式没有真实观测记录", "返回真实连接后读取。"));
    // The app's DOM patcher preserves this region while snapshots update.
    if (this.root) {
      this.updateStatus();
      if (this.root.isConnected && this.active) return el("div", { "data-live-region": "project-logs", "data-patch-key": `project-logs:${id}` });
      return this.root;
    }
    this.headingTarget = el("span", {}, this.targetId ?? id);
    this.status = el("div", { className: "project-logs-status", role: "status" });
    this.notice = el("div", {});
    this.toggle = actionButton("关闭实时更新", { onClick: () => this.toggleLive() });
    this.latest = actionButton("重新读取记录流", { onClick: () => this.restartStream() });
    this.follow = el("input", { type: "checkbox", checked: true, "aria-label": "自动滚动至最新记录" });
    this.logStream = el("div", { className: "project-log-stream", role: "region", tabIndex: 0, "aria-label": "观测记录" });
    this.errorStream = el("div", { className: "project-log-stream project-log-error", role: "region", tabIndex: 0, "aria-label": "来源标记的异常记录" });
    this.root = el("div", { className: "grid", "data-live-region": "project-logs", "data-patch-key": `project-logs:${id}` },
      pageHeader("Observation records", el("span", {}, "观测记录 · ", this.headingTarget), "进入页面后只更新记录流与异常记录。暂停和离开页面会取消当前读取。", [linkButton("返回监控详情", `#/monitoring/monitors/${encodeURIComponent(id)}`), linkButton("监控与故障", "#/monitoring")]),
      el("div", { className: "project-logs-controls" }, this.toggle, this.latest, el("label", {}, this.follow, " 跟随最新记录")),
      this.status, this.notice,
      el("div", { className: "project-logs-grid" },
        card("观测记录", "从可信来源有界读取记录；来源可对内容脱敏。", this.logStream, { className: "project-log-panel" }),
        card("异常记录", "由可信配置分类的记录；来源失败单独显示。", this.errorStream, { className: "project-log-panel" }),
      ),
    );
    this.drawEntries(this.logStream, this.items, false);
    this.drawEntries(this.errorStream, this.errors, true);
    this.updateStatus();
    return this.root;
  }
  revokeAccess() {
    this.cancelRead();
    this.items = [];
    this.errors = [];
    this.seen.clear();
    this.cursor = "";
    this.readAt = null;
    this.streamLabel = null;
    this.targetId = null;
    this.loaded = false;
    this.error = null;
    this.sourceError = null;
    this.coverage = null;
    this.availableStreams = [];
    this.dropped = 0;
    this.drawEntries(this.logStream, [], false);
    this.drawEntries(this.errorStream, [], true);
  }
  activate(id) {
    if (!this.authorized() && (this.loaded || this.items.length || this.errors.length)) this.revokeAccess();
    if (this.active && this.id === id) {
      if (!this.authorized()) this.cancelRead();
      else if (!this.timer && !this.reading && !this.paused && !this.stopped && !document.hidden) this.schedule(0);
      this.updateStatus();
      return;
    }
    if (this.id !== id) this.render(id);
    this.active = true;
    this.paused = false;
    this.visibilityListener = () => {
      this.cancelRead();
      if (!document.hidden && !this.paused && !this.stopped) this.schedule(0);
      this.updateStatus();
    };
    document.addEventListener("visibilitychange", this.visibilityListener);
    if (!document.hidden && !this.stopped) this.schedule(0);
    this.updateStatus();
  }
  deactivate() {
    this.active = false;
    this.cancelRead();
    if (this.visibilityListener) document.removeEventListener("visibilitychange", this.visibilityListener);
    this.visibilityListener = null;
  }
  cancelRead() {
    this.generation = (this.generation ?? 0) + 1;
    clearTimeout(this.timer);
    this.timer = null;
    this.controller?.abort();
    this.controller = null;
    this.reading = Boolean(this.inFlight);
  }
  toggleLive() {
    this.paused = !this.paused;
    this.cancelRead();
    if (!this.paused && !this.stopped && !document.hidden) this.schedule(0);
    this.updateStatus();
  }
  restartStream() {
    if (!this.active || !this.authorized() || !this.api.configured) return;
    this.cancelRead();
    this.cursor = "";
    this.items = [];
    this.errors = [];
    this.seen.clear();
    this.loaded = false;
    this.readAt = null;
    this.streamLabel = null;
    this.error = null;
    this.sourceError = null;
    this.coverage = null;
    this.availableStreams = [];
    this.dropped = 0;
    this.failures = 0;
    this.stopped = false;
    this.paused = false;
    this.drawEntries(this.logStream, [], false);
    this.drawEntries(this.errorStream, [], true);
    if (!document.hidden) this.schedule(0);
    this.updateStatus();
  }
  schedule(delay) {
    if (!this.active || this.paused || this.stopped || document.hidden) return;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => { this.timer = null; void this.read(); }, delay);
  }
  validate(result) {
    if (!result || !Array.isArray(result.items) || !Array.isArray(result.errors) || result.items.length > 32 || result.errors.length > 32
      || (result.next_cursor !== null && result.next_cursor !== undefined && typeof result.next_cursor !== "string")
      || typeof result.has_more !== "boolean") throw new ApiError("invalid_response", "观测记录响应结构不兼容，已停止实时读取。");
    if (result.monitor_id !== undefined && result.monitor_id !== this.id) throw new ApiError("invalid_response", "观测记录响应与当前监控目标不匹配，已停止读取。");
    for (const entry of [...result.items, ...result.errors]) {
      if (!entry || typeof entry.id !== "string" || !entry.id || entry.id.length > 512 || typeof entry.message !== "string"
        || (entry.level !== undefined && typeof entry.level !== "string")) throw new ApiError("invalid_response", "观测记录条目结构不兼容，已停止实时读取。");
    }
    if (result.has_more && (!result.next_cursor || result.next_cursor === this.cursor)) throw new ApiError("invalid_response", "记录游标没有前进，已停止读取以避免重复请求。");
  }
  async read() {
    if (this.reading || !this.active || this.paused || this.stopped || document.hidden) return;
    if (!this.authorized() || !this.api.configured) { this.updateStatus(); return; }
    const generation = this.generation;
    const controller = new AbortController();
    this.controller = controller;
    this.inFlight = controller;
    this.reading = true;
    this.updateStatus();
    let delay = READ_INTERVAL;
    try {
      const result = await this.api.projectLogs(this.id, { cursor: this.cursor, limit: 32, signal: controller.signal });
      if (generation !== this.generation || controller.signal.aborted || !this.authorized()) return;
      this.validate(result);
      const added = this.ingest(result.items, this.items, "record", MAX_LOGS);
      const errors = this.ingest(result.errors, this.errors, "error", MAX_ERRORS);
      this.appendEntries(this.logStream, added, this.items, false);
      this.appendEntries(this.errorStream, errors, this.errors, true);
      this.cursor = result.next_cursor ?? this.cursor;
      this.readAt = result.read_at;
      this.streamLabel = typeof result.stream_label === "string" ? result.stream_label : null;
      this.targetId = typeof result.target_id === "string" ? result.target_id : this.targetId;
      this.sourceError = result.source_error ?? null;
      this.coverage = result.coverage ?? null;
      this.availableStreams = Array.isArray(result.available_streams) ? result.available_streams.slice(0, 8) : [];
      this.error = null;
      this.failures = 0;
      this.loaded = true;
      delay = result.has_more ? 100 : READ_INTERVAL;
    } catch (error) {
      if (generation !== this.generation || controller.signal.aborted) return;
      this.error = error;
      this.failures += 1;
      this.stopped = terminalCodes.has(error.code) || [401, 403, 404].includes(error.status);
      delay = Math.min(30000, READ_INTERVAL * 2 ** Math.min(this.failures, 4));
      if ([401, 403].includes(error.status)) this.onAuthError(error);
    } finally {
      if (this.inFlight === controller) { this.inFlight = null; this.reading = false; }
      if (generation === this.generation) {
        this.controller = null;
        this.updateStatus();
        this.schedule(delay);
      } else {
        // Wait for cancellation to settle before issuing another read.
        this.updateStatus();
        this.schedule(0);
      }
    }
  }
  ingest(entries, collection, kind, max) {
    const added = [];
    for (const entry of entries) {
      const key = `${kind}:${entry.id}`;
      if (this.seen.has(key)) continue;
      this.seen.add(key);
      const value = { id: entry.id, timestamp: entry.timestamp, level: (entry.level ?? "").slice(0, 32), message: entry.message.length > MAX_MESSAGE ? `${entry.message.slice(0, MAX_MESSAGE)}\n[此条记录超过界面显示上限]` : entry.message };
      collection.push(value);
      added.push(value);
    }
    let chars = collection.reduce((total, entry) => total + entry.message.length, 0);
    while (collection.length > max || chars > MAX_TEXT) { chars -= collection.shift().message.length; this.dropped += 1; }
    while (this.seen.size > (MAX_LOGS + MAX_ERRORS) * 2) this.seen.delete(this.seen.values().next().value);
    return added;
  }
  entryNode(entry, error) {
    return el("div", { className: `project-log-row${error ? " project-log-error" : ""}`, "data-log-id": entry.id },
      el("div", { className: "project-log-meta" }, formatDate(entry.timestamp), " · ", entry.level || "未提供级别"),
      el("pre", { className: "project-log-message" }, entry.message));
  }
  drawEntries(stream, entries, error) {
    if (!stream) return;
    stream.replaceChildren(...entries.map((entry) => this.entryNode(entry, error)));
    if (!entries.length) stream.append(el("p", { className: "muted", "data-log-empty": "true" }, error ? "尚未读取到异常记录。" : "尚未读取观测记录。"));
  }
  appendEntries(stream, added, collection, error) {
    if (!stream || !added.length) return;
    const wasAtEnd = stream.scrollHeight - stream.scrollTop - stream.clientHeight < 48;
    stream.querySelector("[data-log-empty]")?.remove();
    const retained = new Set(collection.map((entry) => entry.id));
    for (const entry of added) if (retained.has(entry.id)) stream.append(this.entryNode(entry, error));
    for (const row of [...stream.children]) if (!retained.has(row.dataset.logId)) row.remove();
    // Reading never moves the main page or resets a user's text selection.
    if (this.follow?.checked && wasAtEnd) stream.scrollTop = stream.scrollHeight;
  }
  updateStatus() {
    if (!this.status) return;
    if (this.headingTarget) this.headingTarget.textContent = this.targetId ?? this.id;
    const authorized = this.authorized();
    this.toggle.textContent = this.paused ? "恢复实时更新" : "关闭实时更新";
    this.toggle.disabled = !authorized || this.stopped;
    this.latest.disabled = !authorized || !this.api.configured || this.reading;
    const state = !authorized ? "没有观测记录读取权限" : this.stopped ? "实时读取已停止" : this.paused ? "实时更新已关闭" : document.hidden ? "页面不可见，实时读取已暂停" : this.reading ? "正在读取…" : this.error ? "连接中断，等待重新读取" : this.active ? "实时更新中" : "实时读取已暂停";
    this.status.textContent = `${state} · 记录 ${this.items.length} 条 · 异常 ${this.errors.length} 条 · 本页最近读取 ${this.readAt ? formatDate(this.readAt) : "尚未读取"}${this.streamLabel ? ` · ${this.streamLabel}` : ""}${this.dropped ? ` · 已移除 ${this.dropped} 条较早记录` : ""}`;
    this.notice.replaceChildren(...[
      !authorized ? callout("warning", "无法读取观测记录", "需要监控、日志和扩展读取权限；保留内容仅为此前读取的快照。") : null,
      this.error ? callout("warning", this.error.code === "cursor_invalid" ? "记录游标已失效" : this.error.code === "logs_unavailable" ? "观测记录来源不可用" : "观测记录读取失败", `${this.error.message} ${this.loaded ? "保留上次读取的内容，数据可能陈旧。" : "没有可显示的真实记录。"}${this.stopped ? "可显式重新读取记录流。" : "只读请求将有限退避后重新读取。"}`) : null,
      this.sourceError ? callout("warning", "记录来源提示", sourceStatusMessage(this.sourceError)) : null,
      this.coverage && this.coverage !== "complete" ? callout("warning", "观测范围不完整", this.coverage === "partial" ? "部分记录未能完整读取，请结合来源提示判断。" : "来源报告了非完整观测范围。") : null,
      this.availableStreams.length ? callout("info", "来源报告其他可用记录流", this.availableStreams.join("、")) : null,
    ].filter(Boolean));
    const emptyLogs = this.logStream?.querySelector("[data-log-empty]");
    const emptyErrors = this.errorStream?.querySelector("[data-log-empty]");
    if (this.loaded && !this.items.length && emptyLogs) emptyLogs.textContent = "本次读取范围内没有观测记录。";
    if (this.loaded && !this.errors.length && emptyErrors) emptyErrors.textContent = "当前读取范围内没有异常记录。";
  }
}
