import { el, icon, safeSegment, formatDate, utf8Bytes, bindUtf8Limit, statusChip, tag, linkButton, actionButton, disabledAction, showToast, copyable, detailList, card, callout, pageHeader, sectionHeader, section, metricCard, table, textFrame, diffViewer, jsonDetails, businessVerificationChip } from "./dom.js";
import { HistoryStore } from "./history.js";
import { MonitoringConsole } from "./monitoring.js";
import { ProjectLogsConsole } from "./project-logs.js";
import { patchNode } from "./refresh.js";
import * as demoData from "./data.js";
import { applyShellMetadata, getShellContext } from "./shell.js";
import { ApiClient, ApiError, pollSnapshots } from "./api.js";

const api = new ApiClient();
const historyData = new HistoryStore(api);
const monitoring = new MonitoringConsole({ api, history: historyData, can: canRead, isConnected: () => connection.status === "connected", isDemo, onDiagnose: prepareDiagnosis, onChange: () => renderIfIdle() });
const projectLogs = new ProjectLogsConsole({ api, can: canRead, isDemo, isConnected: () => connection.status === "connected", onAuthError: requireAuthentication });
let approvalCounts = {};
const appMeta = { name: "Recuvora", tagline: "可审查的 AI 恢复控制台", version: "0.1.0", uiVersion: "service-1", lastUpdated: null, operatorMode: "未连接" };
let activeConfiguration = null;
let repairs = [];
let approvals = [];
let capabilities = [];
let harnesses = [];
let simulationTasks = [];
let extensionStatuses = [];
let runtime = {};
let snapshotController = null;
let pageController = null;
let connectionGeneration = 0;
let mountedRoute = null;
let mountedMode = null;
const drafts = new Map();
const listRefreshers = new Map();
let diagnosisContext = null;
let intendedRoute = "#/overview";
let authenticating = false;
const connection = { mode: "live", status: "unconfigured", message: "请输入访问密码，通过认证后进入控制台。", updatedAt: null, hasSnapshot: false, error: null };
api.onUnauthorized = (error) => { if (connection.hasSnapshot) requireAuthentication(error); };
const pendingRequests = new Set();
const operationViews = new Map();
const projectViews = new Map();
const logView = { items: [], nextCursor: null, loaded: false, loading: false, error: null, readAt: null, filters: { level: "", source: "", task_id: "", operation_id: "", query: "", limit: 50 }, cursor: "", history: [] };
const navigation = [
  { id: "overview", label: "概览", href: "#/overview", icon: "overview" },
  { id: "monitoring", label: "监控与故障", href: "#/monitoring", icon: "activity" },
  { id: "repairs", label: "修复任务", href: "#/repairs", icon: "repair" },
  { id: "approvals", label: "审批", href: "#/approvals", icon: "approval" },
  { id: "harnesses", label: "AI Harness", href: "#/harnesses", icon: "harness" },
  { id: "logs", label: "日志查询", href: "#/logs", icon: "terminal" },
  { id: "simulation", label: "模拟实验室", href: "#/simulation", icon: "simulation" },
  { id: "capabilities", label: "能力中心", href: "#/capabilities", icon: "capability" },
];

function isDemo() { return connection.mode === "demo"; }
function defaultServiceBase() {
  const configured = String(api.baseUrl || "").trim();
  if (/^https?:\/\//i.test(configured)) return configured;
  if (shell.kind === "web" && ["http:", "https:"].includes(location.protocol)) return location.origin;
  return "";
}
function can(permission) {
  return !isDemo() && connection.status === "connected" && runtime.permissions?.includes(permission) === true;
}
function canRead(permission) {
  return !isDemo() && connection.hasSnapshot && !["unauthorized", "unconfigured", "connecting"].includes(connection.status) && runtime.permissions?.includes(permission) === true;
}
function configuredCapability(id) {
  return capabilities.some((item) => item.id === id && item.availability === "available");
}
function resetViews() {
  historyData.clear();
  monitoring.clear();
  projectLogs.clear();
  approvalCounts = {};
  activeConfiguration = null;
  repairs = []; approvals = []; capabilities = []; harnesses = []; simulationTasks = [];
  extensionStatuses = [];
  runtime = {};
  connection.hasSnapshot = false;
  connection.updatedAt = null;
  connection.error = null;
  drafts.clear();
  diagnosisContext = null;
  mountedRoute = null;
  operationViews.clear(); projectViews.clear(); pendingRequests.clear();
  Object.assign(logView, { items: [], nextCursor: null, loaded: false, loading: false, error: null, readAt: null, cursor: "", history: [] });
}
function applySnapshot(snapshot) {
  if (snapshot.schema_version !== 1 || snapshot.mode !== "live" || !snapshot.runtime || !["capabilities", "harnesses", "repairs", "approvals", "simulation_tasks"].every((key) => Array.isArray(snapshot[key]))) {
    throw new ApiError("invalid_response", "服务快照版本或结构不兼容；没有使用演示数据补齐。");
  }
  historyData.seed(snapshot);
  approvalCounts = snapshot.approval_counts ?? {};
  runtime = snapshot.runtime;
  monitoring.seed(snapshot.monitoring ?? null, runtime.permissions ?? []);
  activeConfiguration = snapshot.active_configuration ?? null;
  repairs = snapshot.repairs.map((item) => ({ ...item, operationIds: item.operationIds ?? [] }));
  approvals = snapshot.approvals.map((item) => ({ ...item, policy: { ...item.policy, allowedTargets: item.policy?.allowedTargets ?? [], allowedActions: item.policy?.allowedActions ?? [] }, executionContext: item.executionContext ?? {} }));
  capabilities = snapshot.capabilities;
  harnesses = snapshot.harnesses.map((item) => ({ ...item, workspaceRoots: item.workspaceRoots ?? [] }));
  simulationTasks = snapshot.simulation_tasks;
  extensionStatuses = snapshot.extension_statuses ?? [];
  for (const operation of snapshot.operations ?? []) {
    if (operationViews.has(operation.id)) operationViews.set(operation.id, { ...operationViews.get(operation.id), ...operation });
  }
  connection.hasSnapshot = true;
  connection.updatedAt = runtime.updatedAt ?? null;
  appMeta.lastUpdated = connection.updatedAt;
  appMeta.operatorMode = runtime.operator ?? "已认证操作员";
  connection.status = "connected";
  connection.message = "已认证，正在更新运行状态。";
  connection.error = null;
  const route = parseRoute();
  monitoring.refreshActive(route);
  if (route.section === "overview" && can("incident.read")) {
    const page = historyData.page("overview-incidents");
    if (page.loaded && !page.loading) historyData.load("overview-incidents", { filters: { status: "active" }, signal: snapshotController?.signal }).then(renderIfIdle);
  }
  if (["repairs", "approvals"].includes(route.section) && route.id && route.id !== "new") currentDetail(route.section, route.id);
}
function renderIfIdle() {
  renderApp({ focus: false });
}
function startPolling() {
  snapshotController?.abort();
  if (!api.configured || isDemo() || document.hidden) return;
  const controller = new AbortController();
  snapshotController = controller;
  const generation = connectionGeneration;
  pollSnapshots(api, {
    signal: controller.signal,
    onSnapshot: (snapshot) => {
      if (generation !== connectionGeneration) return;
      applySnapshot(snapshot);
      renderIfIdle();
    },
    onError: (error) => {
      if (generation !== connectionGeneration) return;
      if (error.status === 401) { requireAuthentication(error); return; }
      connection.status = [401, 403].includes(error.status) ? "unauthorized" : "offline";
      connection.error = error;
      connection.message = error.message;
      renderIfIdle();
    },
  }).then(() => {
    if (!controller.signal.aborted && generation === connectionGeneration) {
      renderIfIdle();
    }
  }).catch((error) => {
    if (controller.signal.aborted || generation !== connectionGeneration) return;
    connection.status = "offline";
    connection.message = error.message;
    renderIfIdle();
  });
}
function selectDemo(enabled) {
  snapshotController?.abort(); pageController?.abort(); api.disconnect();
  connectionGeneration++;
  resetViews();
  connection.mode = enabled ? "demo" : "live";
  connection.status = enabled ? "demo" : "unconfigured";
  connection.message = enabled ? "演示界面不会提交操作。" : "请输入访问密码，通过认证后进入控制台。";
  appMeta.operatorMode = enabled ? demoData.appMeta.operatorMode : "未连接";
  if (enabled) {
    activeConfiguration = demoData.activeConfiguration;
    repairs = demoData.repairs; approvals = demoData.approvals; capabilities = demoData.capabilities;
    harnesses = demoData.harnesses; simulationTasks = demoData.simulationTasks;
    connection.updatedAt = demoData.appMeta.lastUpdated;
    appMeta.lastUpdated = demoData.appMeta.lastUpdated;
    connection.hasSnapshot = true;
  }
  renderApp({ focus: false });
}

function requireAuthentication(error) {
  intendedRoute = location.hash && location.hash !== "#/auth" ? location.hash : intendedRoute;
  snapshotController?.abort(); pageController?.abort(); api.disconnect();
  connectionGeneration++;
  resetViews();
  connection.mode = "live";
  connection.status = "unauthorized";
  connection.message = error?.message ?? "认证已失效，请重新输入访问密码。";
  location.hash = "#/auth";
  renderApp({ focus: false });
}

function saveDrafts() {
  if (!mountedRoute || mountedRoute === "auth") return;
  const values = {};
  for (const control of document.querySelectorAll("main input, main textarea, main select")) {
    if (control.id && control.type !== "password" && !control.disabled) values[control.id] = control.value;
  }
  if (Object.keys(values).length) drafts.set(mountedRoute, values);
  while (drafts.size > 12) drafts.delete(drafts.keys().next().value);
}

function restoreDrafts(path) {
  for (const [id, value] of Object.entries(drafts.get(path) ?? {})) {
    const control = document.getElementById(id);
    if (control && !control.disabled && (control.tagName !== "SELECT" || [...control.options].some((option) => option.value === value))) {
      control.value = value;
      control.dispatchEvent(new Event("input", { bubbles: true }));
      if (control.tagName === "SELECT") control.dispatchEvent(new Event("change", { bubbles: true }));
    }
  }
}

function prepareDiagnosis(record) {
  diagnosisContext = { incidentId: record.id, revision: record.revision, target: record.target_id, summary: record.summary, evidence: record.evidence };
  const sameTarget = activeConfiguration?.targetId === record.target_id;
  if (sameTarget) {
    drafts.set("/repairs/new", { "repair-prompt": `调查故障 ${record.id}：${record.summary}\n目标：${record.target_id}\n请先诊断证据，仅提出已授权范围内的准确操作。` });
    location.hash = "#/repairs/new";
  } else {
    showToast("已保留诊断上下文；当前修复配置未覆盖此目标，请先配置相应能力。");
    location.hash = "#/harnesses";
  }
}

function diagnosisCard() {
  if (!diagnosisContext) return null;
  const context = diagnosisContext;
  return el("section", { className: "section", dataset: { patchKey: "diagnosis-context" } }, card("故障诊断上下文", `${context.target} · ${context.summary}`,
    el("div", { className: "grid" },
      detailList([["故障", copyable(context.incidentId)], ["证据版本", String(context.revision)], ["目标", context.target]]),
      jsonDetails("查看故障证据", context.evidence),
      el("p", { className: "muted" }, activeConfiguration?.targetId === context.target ? "提交修复时宿主会核对故障版本和目标范围。" : "当前修复配置未覆盖此目标。可以先调查证据，修复需配置对应目标。"),
      el("div", { className: "action-bar-buttons" }, linkButton("返回故障", `#/monitoring/incidents/${safeSegment(context.incidentId)}`),
        actionButton("清除诊断上下文", { onClick: () => { diagnosisContext = null; renderIfIdle(); } })),
    )));
}

const shell = getShellContext();
applyShellMetadata(shell);

const app = document.getElementById("app");
const routeAnnouncer = document.getElementById("route-announcer");

const viewState = {
  navOpen: false,
  repairFilter: "all",
  repairSearch: "",
  approvalFilter: "attention",
  approvalSearch: "",
  capabilityFilter: "all",
  capabilityAvailability: "all",
};

const repairStates = Object.freeze({
  completed: { label: "流程已结束", tone: "success", description: "本轮流程已结束；文件执行事实请核对审批记录，业务恢复尚未验证。" },
  waiting_human: { label: "等待人工决定", tone: "warning", description: "存在需要本机操作员审查的具体操作。" },
  blocked: { label: "流程受阻", tone: "danger", description: "审批拒绝、过期、撤销或操作失败阻止继续执行。" },
  unknown: { label: "结果未知", tone: "unknown", description: "操作可能已发生，必须先核验目标。" },
  canceled: { label: "本轮已取消", tone: "neutral", description: "取消不应被解释为已经撤销外部副作用。" },
  failed: { label: "本轮失败", tone: "danger", description: "流程明确失败，没有自动重试。" },
  running: { label: "运行中", tone: "primary", description: "正在等待权威运行时结果。" },
});

const approvalStates = Object.freeze({
  pending: { label: "等待评审", tone: "info" },
  waiting_human: { label: "待人工决定", tone: "warning" },
  approved: { label: "已批准，待执行", tone: "primary" },
  denied: { label: "已拒绝", tone: "danger" },
  revoked: { label: "已撤销", tone: "neutral" },
  canceled: { label: "已取消", tone: "neutral" },
  expired: { label: "已过期", tone: "neutral" },
  executing: { label: "正在执行", tone: "primary" },
  executed: { label: "已执行", tone: "success" },
  failed: { label: "执行失败", tone: "danger" },
  unknown: { label: "结果未知", tone: "unknown" },
});

const simulationStates = Object.freeze({
  running: { label: "模拟运行中", tone: "primary" },
  accepted: { label: "模拟已接受", tone: "info" },
  queued: { label: "模拟排队", tone: "info" },
  diagnosing: { label: "模拟诊断中", tone: "primary" },
  executing: { label: "模拟执行中", tone: "primary" },
  verifying: { label: "模拟验证中", tone: "primary" },
  succeeded: { label: "模拟成功", tone: "success" },
  failed: { label: "模拟失败", tone: "danger" },
  denied: { label: "模拟未授权", tone: "danger" },
  canceled: { label: "模拟已取消", tone: "neutral" },
  timed_out: { label: "模拟超时", tone: "warning" },
  unknown: { label: "模拟结果未知", tone: "unknown" },
});

const capabilityStates = Object.freeze({
  implemented: { label: "已实现", tone: "success" },
  prototype: { label: "原型实现", tone: "info" },
  limited: { label: "受限实现", tone: "warning" },
  in_progress: { label: "开发中", tone: "primary" },
  planned: { label: "规划中", tone: "neutral" },
  not_implemented: { label: "未实现", tone: "neutral" },
  unconfigured: { label: "未配置", tone: "warning" },
  unavailable: { label: "不可用", tone: "danger" },
});

const harnessAvailability = Object.freeze({
  configured: { label: "已配置（未探测）", tone: "info" },
  disabled: { label: "已停用", tone: "neutral" },
  unavailable: { label: "装配不可用", tone: "danger" },
});

function parseRoute() {
  const raw = location.hash.replace(/^#/, "") || "/overview";
  const [pathname, query = ""] = raw.split("?", 2);
  const clean = pathname.startsWith("/") ? pathname : `/${pathname}`;
  const parts = clean.split("/").filter(Boolean).map((part) => {
    try {
      return decodeURIComponent(part);
    } catch {
      return part;
    }
  });
  return {
    path: clean,
    section: parts[0] || "overview",
    id: parts[1] || null,
    action: parts[2] || null,
    query: new URLSearchParams(query),
  };
}

function previewDisabledReason() {
  return el(
    "div",
    { className: "form-footer-copy" },
    icon("lock", 13),
    isDemo() ? " 演示数据不会提交操作。" : connection.status === "connected" ? " 请求由可信服务重新校验；发送不代表执行成功。" : " 当前未连接可信服务，操作不可提交。",
  );
}

function routeTitle(route) {
  const titles = {
    overview: "概览",
    repairs: route.id === "new" ? "启动受限修复" : route.id ? "修复任务详情" : "修复任务",
    approvals: route.id ? "审批详情" : "审批",
    monitoring: route.id === "logs" ? "观测记录" : route.id === "incidents" ? "故障详情" : route.id === "monitors" ? "监控详情" : route.id === "plugins" ? "插件监控" : "监控与故障",
    harnesses: route.action === "run" ? "Harness 文本轮次" : route.id ? "Harness 详情" : "Harness",
    simulation: "模拟实验室",
    logs: "日志查询",
    capabilities: route.id ? "能力详情" : "能力中心",
    settings: "设置",
    about: "关于",
    auth: "认证",
  };
  return titles[route.section] ?? "未找到页面";
}

function navigationLink(item, activeSection) {
  const anchor = el(
    "a",
    {
      className: "nav-link",
      href: item.href,
      "aria-current": activeSection === item.id ? "page" : null,
      on: {
        click: () => {
          viewState.navOpen = false;
          const layout = document.querySelector(".app-layout");
          if (layout) layout.dataset.navOpen = "false";
        },
      },
    },
    el("span", { className: "nav-icon" }, icon(item.icon, 18)),
    el("span", { className: "nav-link-label" }, item.label),
    item.count ? el("span", { className: "nav-count", "aria-label": `${item.count} 项需关注` }, item.count) : null,
  );
  return el("li", {}, anchor);
}

function buildSidebar(route) {
  const systemNavigation = [
    { id: "settings", label: "设置", href: "#/settings", icon: "settings" },
    { id: "about", label: "关于", href: "#/about", icon: "info" },
  ];
  return el(
    "aside",
    { className: "sidebar", "aria-label": "主导航" },
    el(
      "a",
      { className: "sidebar-brand", href: "#/overview", "aria-label": "Recuvora 概览" },
      el("span", { className: "brand-mark" }, icon("activity", 20)),
      el(
        "span",
        { className: "brand-copy" },
        el("span", { className: "brand-name" }, appMeta.name),
        el("span", { className: "brand-tagline" }, appMeta.tagline),
      ),
    ),
    el(
      "div",
      { className: "sidebar-body" },
      el("div", { className: "nav-label" }, "日常运行"),
      el("ul", { className: "nav-list" }, navigation.filter((item) => ["overview", "monitoring", "repairs", "approvals"].includes(item.id)).map((item) => navigationLink(item, route.section))),
      el("div", { className: "nav-label" }, "调查与工具"),
      el("ul", { className: "nav-list" }, navigation.filter((item) => ["harnesses", "logs", "simulation"].includes(item.id)).map((item) => navigationLink(item, route.section))),
      el("div", { className: "nav-label" }, "系统管理"),
      el("ul", { className: "nav-list" }, [...navigation.filter((item) => item.id === "capabilities"), ...systemNavigation].map((item) => navigationLink(item, route.section))),
    ),
    el(
      "div",
      { className: "sidebar-footer" },
      el(
        "div",
        { className: "operator-card" },
        el("span", { className: "operator-avatar" }, icon("shield", 15)),
        el(
          "div",
          {},
          el("div", { className: "operator-title" }, appMeta.operatorMode),
          el("div", { className: "operator-detail" }, isDemo() ? "演示身份" : "权限由可信服务核验"),
        ),
      ),
    ),
  );
}

function buildTopbar(route) {
  const title = routeTitle(route);
  return el(
    "header",
    { className: "topbar" },
    el(
      "div",
      { className: "topbar-left" },
      actionButton("", {
        variant: "ghost button-icon menu-button",
        iconName: "menu",
        ariaLabel: "打开导航",
        onClick: () => {
          viewState.navOpen = !viewState.navOpen;
          const layout = document.querySelector(".app-layout");
          if (layout) layout.dataset.navOpen = String(viewState.navOpen);
        },
      }),
      el("div", { className: "breadcrumb" }, "Recuvora", " / ", el("strong", {}, title)),
    ),
    el(
      "div",
      { className: "topbar-right" },
      el(
        "span",
        { className: "freshness-pill", dataset: { status: isDemo() ? "demo" : connection.status }, title: isDemo() ? "演示数据时间" : "最近一次成功读取权威快照的时间" },
        el("span", { className: "freshness-dot", "aria-hidden": "true" }),
        `${isDemo() ? "演示数据" : connection.status === "connected" ? "服务快照" : "未连接 / 陈旧"} · ${connection.updatedAt ? formatDate(connection.updatedAt) : "尚无数据"}`,
      ),
      el(
        "span",
        { className: "shell-pill" },
        icon(shell.kind === "desktop" ? "terminal" : "external", 14),
        el("span", {}, shell.kind === "desktop" ? "Desktop" : "Web"),
      ),
    ),
  );
}

function buildPreviewBanner() {
  const title = isDemo() ? "UI 预览 · 演示数据 · 操作不会提交" : connection.status === "connected" ? "真实服务 · 权威快照" : connection.hasSnapshot ? "连接中断或暂停 · 当前快照可能已过期" : "真实模式 · 尚未连接服务";
  return el(
    "div",
    { className: `preview-banner connection-bar ${isDemo() ? "connection-bar-demo" : connection.status === "connected" ? "connection-bar-live" : "connection-bar-warning"}`, role: "status", dataset: { connectionState: connection.status } },
    el("span", { className: "preview-banner-icon" }, icon(connection.status === "connected" ? "check" : "warning", 18)),
    el(
      "div",
      { className: "preview-banner-content" },
      el("div", { className: "preview-banner-title" }, title),
      el(
        "div",
        { className: "preview-banner-copy" },
        isDemo() ? "当前明确使用演示数据；批准、拒绝、撤销、执行、核验、Harness 调用与模拟运行均已禁用。" : connection.message,
      ),
    ),
    linkButton("连接设置", "#/settings", { iconName: "settings" }),
  );
}

function renderPage(route) {
  if (!connection.hasSnapshot && !["settings", "about", "logs"].includes(route.section)) {
    return el("div", {}, pageHeader("Runtime connection", routeTitle(route), "此页面需要可信服务提供数据；没有用演示数据填补。"), renderConnectionPanel());
  }
  switch (route.section) {
    case "overview": return renderOverview();
    case "monitoring": return route.id === "logs" && route.action ? projectLogs.render(route.action) : monitoring.render(route);
    case "repairs": return route.id === "new" ? renderNewRepair() : route.id ? renderRepairDetail(route.id) : renderRepairs();
    case "approvals": return route.id ? renderApprovalDetail(route.id) : renderApprovals();
    case "harnesses": return route.id && route.action === "run" ? renderHarnessRun(route.id) : route.id ? renderHarnessDetail(route.id) : renderHarnesses();
    case "simulation": return renderSimulation();
    case "logs": return renderLogs();
    case "capabilities": return route.id ? renderCapabilityDetail(route.id) : renderCapabilities();
    case "settings": return renderSettings();
    case "about": return renderAbout();
    default: return renderNotFound();
  }
}

function renderApp(options = {}) {
  const route = parseRoute();
  const auth = !connection.hasSnapshot && !isDemo();
  const path = auth ? "auth" : route.path + (route.query.toString() ? `?${route.query}` : "");
  const mode = auth ? "auth" : connection.mode;
  if (auth) {
    if (route.section !== "auth") {
      intendedRoute = location.hash || "#/overview";
      history.replaceState(null, "", "#/auth");
    }
    const view = renderAuthentication();
    if (mountedRoute === path && mountedMode === mode && app.firstChild) patchNode(app.firstChild, view);
    else { projectLogs.deactivate(); listRefreshers.clear(); app.replaceChildren(view); }
    mountedRoute = path; mountedMode = mode;
    return;
  }
  if (route.section === "auth") {
    location.hash = intendedRoute === "#/auth" ? "#/overview" : intendedRoute;
    return;
  }
  const samePage = mountedRoute === path && mountedMode === mode;
  if (samePage) {
    for (const [selector, next] of [[".topbar", buildTopbar(route)], [".preview-banner", buildPreviewBanner()], [".sidebar", buildSidebar(route)]]) {
      const current = document.querySelector(selector);
      if (current) patchNode(current, next);
    }
    const current = document.getElementById("page-view");
    if (current) patchNode(current, el("div", { id: "page-view" }, renderPage(route)));
    for (const refresh of listRefreshers.values()) refresh.draw();
    const operations = document.getElementById("operation-results");
    if (operations) patchNode(operations, el("div", { id: "operation-results" }, renderOperationViews()));
    synchronizeActions();
    if (route.section === "monitoring" && route.id === "logs") projectLogs.activate(route.action);
    return;
  }
  saveDrafts();
  projectLogs.deactivate();
  listRefreshers.clear();
  const layout = el("div", { className: "app-layout", dataset: { navOpen: viewState.navOpen } });
  layout.append(buildSidebar(route));
  layout.append(
    el("button", {
      className: "mobile-scrim",
      type: "button",
      "aria-label": "关闭导航",
      on: {
        click: () => {
          viewState.navOpen = false;
          layout.dataset.navOpen = "false";
        },
      },
    }),
  );
  const main = el(
    "div",
    { className: "main-shell" },
    buildTopbar(route),
    buildPreviewBanner(),
    el("main", { id: "main-content", className: "page-container", tabIndex: -1 }, el("div", { id: "page-view" }, renderPage(route)), el("div", { id: "operation-results" }, renderOperationViews())),
  );
  layout.append(main);
  layout.append(el("div", { className: "toast-region", "aria-live": "polite", "aria-atomic": "true" }));
  app.replaceChildren(layout);
  mountedRoute = path; mountedMode = mode;
  restoreDrafts(path);
  synchronizeActions();
  if (route.section === "monitoring" && route.id === "logs") projectLogs.activate(route.action);
  if (routeAnnouncer) routeAnnouncer.textContent = `已打开：${routeTitle(route)}`;
  if (options.focus !== false) {
    requestAnimationFrame(() => {
      const heading = document.querySelector("main h1");
      if (heading) heading.focus({ preventScroll: true });
      window.scrollTo({ top: 0, behavior: "auto" });
    });
  }
}

function synchronizeActions() {
  for (const button of document.querySelectorAll("[data-service-action]")) {
    const detail = button.dataset.approvalId ? historyData.detail("approvals", button.dataset.approvalId) : null;
    const missingDependency = button.dataset.serviceAction === "repair.run" ? !configuredCapability("text-repair")
      : button.dataset.serviceAction === "harness.run" ? !configuredCapability("harness") || !harnesses.some((item) => `harness:${item.id}` === button.dataset.requestKey && item.enabled && item.availability === "configured") : false;
    button.disabled = !can(button.dataset.serviceAction) || missingDependency || button.dataset.actionAvailable === "false" || pendingRequests.has(button.dataset.requestKey)
      || Boolean(detail && (detail.stale || String(detail.value?.revision) !== button.dataset.revision));
  }
  monitoring.synchronizeActions();
}

async function authenticateConnection(base, token) {
  if (authenticating) return;
  authenticating = true;
  try {
    snapshotController?.abort(); pageController?.abort();
    api.connect(base.value, token.value);
    token.value = "";
    connectionGeneration++;
    const generation = connectionGeneration;
    resetViews();
    connection.mode = "live";
    connection.status = "connecting";
    connection.message = "正在验证访问密码并读取运行能力……";
    // Keep the authentication form mounted during validation.
    mountedRoute = "auth"; mountedMode = "auth";
    renderApp({ focus: false });
    const snapshot = await api.bootstrap();
    if (generation !== connectionGeneration) return;
    applySnapshot(snapshot);
    mountedRoute = null;
    location.hash = intendedRoute === "#/auth" ? "#/overview" : intendedRoute;
    renderApp();
    startPolling();
  } catch (error) {
    api.disconnect();
    connection.status = error.status === 401 ? "unauthorized" : "unconfigured";
    connection.message = error.status === 401 ? "访问密码不正确，请重新输入。" : error.message;
    renderApp({ focus: false });
  } finally {
    authenticating = false;
    renderApp({ focus: false });
  }
}

function renderAuthentication() {
  const base = el("input", { id: "auth-base", className: "field mono", type: "url", autocomplete: "off", value: defaultServiceBase(), placeholder: "https://host.example", required: true });
  const token = el("input", { id: "auth-password", className: "field", type: "password", autocomplete: "off", required: true, placeholder: "输入宿主提供的访问令牌" });
  const form = el("form", { className: "auth-form", on: { submit: (event) => { event.preventDefault(); if (form.reportValidity()) void authenticateConnection(base, token); } } },
    el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "auth-base" }, "服务地址"), base),
    el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "auth-password" }, "访问密码 / 令牌"), token),
    actionButton(authenticating ? "正在认证……" : "认证并进入控制台", { type: "submit", variant: "primary", disabled: authenticating }),
  );
  return el("main", { className: "auth-page", id: "main-content", tabIndex: -1 }, el("section", { className: "auth-card" },
    el("div", { className: "brand-mark" }, icon("shield", 24)),
    el("h1", {}, "连接 Recuvora"),
    el("p", { className: "page-description" }, "通过宿主认证后进入运行控制台。访问密码只保存在当前页面内存。"),
    form,
    el("p", { className: "auth-message", role: "status" }, connection.message),
    el("p", { className: "form-hint" }, "本机可使用回环 HTTP；远程连接使用 HTTPS。"),
    actionButton("查看演示界面", { variant: "ghost", onClick: () => { intendedRoute = "#/overview"; location.hash = "#/overview"; selectDemo(true); } }),
  ));
}

function overviewAttentionTable() {
  const priority = { unknown: 0, waiting_human: 1, approved: 2, pending: 3 };
  const urgent = approvals.filter((approval) => approval.state in priority).sort((a, b) => priority[a.state] - priority[b.state]);
  return table(
    ["审批请求", "目标 / 文件", "状态", "更新时间", "下一步"],
    urgent.map((approval) => [
      el(
        "div",
        {},
        el("a", { className: "table-link", href: `#/approvals/${safeSegment(approval.requestId)}` }, approval.requestId),
        el("span", { className: "table-secondary" }, approval.taskId),
      ),
      el(
        "div",
        {},
        el("span", { className: "table-primary" }, approval.target),
        el("span", { className: "table-secondary mono" }, approval.path),
      ),
      statusChip(approvalStates[approval.state]),
      el("span", { className: "numeric" }, formatDate(approval.updatedAt, true)),
      el(
        "span",
        { className: "muted" },
        approval.state === "unknown" ? "读取并核验，不要重试" : approval.state === "approved" ? "显式执行或撤销" : "审查准确替换",
      ),
    ]),
    "需要优先处理的审批请求",
  );
}

function activeConfigCard() {
  if (!activeConfiguration) return card("活动配置", "未配置", el("p", { className: "muted" }, "宿主未提供修复配置；相关提交入口保持禁用。"));
  return card(
    "活动配置",
    isDemo() ? "只读演示" : "由可信宿主加载的只读快照",
    detailList([
      ["配置", copyable(activeConfiguration.name, "复制配置名称")],
      ["目标", el("span", { className: "table-primary" }, activeConfiguration.targetId)],
      ["目标根", copyable(activeConfiguration.targetRoot, "复制目标根")],
      ["执行 Harness", el("span", { className: "mono" }, activeConfiguration.executionHarness)],
      ["审批方式", activeConfiguration.reviewer],
      ["允许文件", el("div", { className: "scope-files" }, (activeConfiguration.allowedFiles ?? []).map((file) => tag(file, { mono: true })))],
      ["策略", `${activeConfiguration.policyId} · v${activeConfiguration.policyVersion}`],
    ]),
    {
      headerAction: tag(isDemo() ? "演示" : "服务快照", { demo: isDemo() }),
      footer: el("span", { className: "muted" }, "切换状态目录可能展示另一套记录；不能用来绕过未决操作。"),
    },
  );
}

function capabilitySnapshot() {
  const shown = capabilities.filter((item) => ["simulation", "harness", "text-repair", "business-verification"].includes(item.id));
  return el(
    "div",
    { className: "grid grid-2" },
    shown.map((item) =>
      el(
        "a",
        { className: "card card-interactive capability-card", href: `#/capabilities/${safeSegment(item.id)}` },
        el(
          "div",
          { className: "capability-card-top" },
          el("span", { className: "capability-mark" }, icon(item.id === "simulation" ? "simulation" : item.id === "harness" ? "harness" : "capability", 17)),
          statusChip(capabilityStates[item.state]),
        ),
        el("h3", {}, item.name),
        !isDemo() ? tag(item.availability === "available" ? "运行可用" : item.availability === "unconfigured" ? "未配置" : "运行不可用") : null,
        el("p", { className: "capability-summary" }, item.summary),
        el("div", { className: "capability-limitation" }, item.limitation),
      ),
    ),
  );
}

function renderOverview() {
  const waiting = approvalCounts.waiting_human ?? approvals.filter((item) => item.state === "waiting_human").length;
  const unknown = approvalCounts.unknown ?? approvals.filter((item) => item.state === "unknown").length;
  const active = approvalCounts.approved ?? approvals.filter((item) => item.state === "approved").length;
  const counts = monitoring.snapshot?.counts;
  const targets = new Set((monitoring.snapshot?.monitors ?? []).map((item) => item.target_id)).size;
  return el(
    "div",
    {},
    pageHeader(
      "Control center",
      "运行概览",
      isDemo() ? "演示记录仅用于查看界面。" : "查看监控目标、活动故障和需要处理的操作。",
      [linkButton("查看监控目标", "#/monitoring", { variant: "primary", iconName: "activity" })],
    ),
    section("目标与故障", "采集状态与目标健康分别判断；异常条件解除不代表业务恢复验证。", el("div", { className: "grid grid-4" },
      metricCard("监控目标", isDemo() ? "—" : counts?.targets ?? targets, "查看各目标的监控", "target", "primary", "#/monitoring"),
      metricCard("未解除故障", isDemo() ? "—" : counts?.incidents?.active ?? "待查询", "包括已确认但尚未解除", "warning", "warning", "#/monitoring?status=active"),
      metricCard("采集异常", isDemo() ? "—" : counts?.collection_issues ?? "待查询", "覆盖不足或采集不可用", "activity", "warning", "#/monitoring"),
      metricCard("过期样本", isDemo() ? "—" : counts?.stale ?? "待查询", "判断需要新观测支持", "clock", "unknown", "#/monitoring"),
    )),
    section(
      "待处理操作",
      "先核验结果未知的操作，再处理审批和执行。",
      el(
        "div",
        { className: "grid grid-4" },
        metricCard("结果未知", unknown, "先读取当前结果核验", "warning", "unknown", "#/approvals?state=unknown"),
        metricCard("待人工决定", waiting, "需要审查准确操作", "approval", "warning", "#/approvals?state=waiting_human"),
        metricCard("批准待执行", active, "进入详情执行或撤销", "clock", "primary", "#/approvals?state=approved"),
        metricCard("正在执行", approvalCounts.executing ?? approvals.filter((item) => item.state === "executing").length, "等待操作回执", "activity", "primary", "#/approvals?state=executing"),
      ),
    ),
    section("活动故障", "查看异常证据，确认收到只记录已知悉。", !isDemo() && canRead("incident.read") ? renderHistoryList("overview-incidents", (items) => monitoring.incidentTable(items), null, { status: "active" }) : el("p", { className: "muted" }, isDemo() ? "演示模式不生成真实故障。" : "当前身份无法读取故障。")),
    section("优先处理队列", "先处理结果未知，再处理待审批与批准待执行。", overviewAttentionTable(), el("a", { className: "section-link", href: "#/approvals" }, "查看全部", icon("arrow", 13))),
    section(
      "运行上下文",
      "配置由可信宿主加载；前端只读展示服务快照。",
      el("div", { className: "content-grid" }, activeConfigCard(), card(
        "可靠性语义",
        "界面不得弱化这些边界",
        el(
          "ul",
          { className: "timeline" },
          [
            ["unknown", "结果未知不是失败", "先核验目标，不能盲目重放。"],
            ["check", "批准不等于执行", "人工批准后仍需显式应用。"],
            ["warning", "读回不等于恢复", "content_verified 只证明文本一致。"],
            ["lock", "前端不产生权限", "授权只来自可信宿主持久记录。"],
          ].map(([iconName, title, detail], index) =>
            el(
              "li",
              { className: "timeline-item" },
              el("span", { className: `timeline-node ${index === 0 ? "timeline-node-unknown" : ""}` }, icon(iconName, 13)),
              el("div", { className: "timeline-copy" }, el("div", { className: "timeline-title" }, title), el("div", { className: "timeline-detail" }, detail)),
            ),
          ),
        ),
      )),
    ),
    section("能力快照", "当前能力和规划能力分开显示，不用空数据冒充可用。", capabilitySnapshot(), el("a", { className: "section-link", href: "#/capabilities" }, "打开能力中心", icon("arrow", 13))),
  );
}

function repairTable(items) {
  if (!items.length) {
    return el(
      "div",
      { className: "card empty-state" },
      el("div", {}, el("span", { className: "empty-icon" }, icon("search", 20)), el("h2", {}, "没有匹配的修复任务"), el("p", {}, "调整筛选或搜索词；结果属于页面标示时间的快照。")),
    );
  }
  return table(
    ["任务", "目标", "流程状态", "操作", "审批方式", "更新时间", "业务验证"],
    items.map((repair) => [
      el(
        "div",
        {},
        el("a", { className: "table-link", href: `#/repairs/${safeSegment(repair.id)}` }, repair.id),
        el("span", { className: "table-secondary" }, repair.summary),
      ),
      el("span", { className: "table-primary" }, repair.target),
      statusChip(repairStates[repair.status]),
      el("span", { className: "numeric" }, `${repair.operationCount} 项`),
      repair.reviewer,
      el("span", { className: "numeric" }, formatDate(repair.updatedAt, true)),
      businessVerificationChip(),
    ]),
    "受限修复任务列表",
  );
}

function renderRepairs() {
  return el("div", {}, pageHeader("Repair workflow", "修复任务", "摘要按不可变 ID 降序分页；完整回复在详情页读取。流程结束不代表业务恢复。", [linkButton("启动受限修复", "#/repairs/new", { variant: "primary", iconName: "plus" })]),
    isDemo() ? repairTable(repairs) : renderHistoryList("repairs", repairTable, [["all", "全部"], ["attention", "需关注"], ["completed", "流程结束"], ["failed", "失败"], ["canceled", "已取消"]]));
}

function renderHistoryList(name, renderItems, filters = null, fixed = {}) {
  const widgetKey = `${name}:${JSON.stringify(fixed)}`;
  const existing = listRefreshers.get(widgetKey);
  if (existing) { existing.draw(); return el("div", { dataset: { liveRegion: `history-${widgetKey}` } }); }
  const page = historyData.page(name);
  const routeState = name === "approvals" ? parseRoute().query.get("state") : null;
  if (routeState && page.filters.state !== routeState) { page.filters = { ...page.filters, state: routeState }; page.loaded = false; }
  const host = el("div", {});
  const select = filters ? el("select", { className: "select", "aria-label": "历史状态筛选" }, filters.map(([value, label]) => el("option", { value }, label))) : null;
  if (select) select.value = page.filters.state ?? filters[0][0];
  const search = filters ? el("input", { className: "field", type: "search", maxlength: "256", value: page.filters.query ?? "", "aria-label": "搜索历史摘要", placeholder: "搜索摘要，点击查询" }) : null;
  const draw = () => patchNode(host, el("div", {}, el("div", { className: "grid" },
    page.error ? callout("warning", "历史读取失败", `${page.error.message} 保留上次页面，不代表空结果。`) : null,
    page.loaded && Object.entries(fixed).every(([key, value]) => page.filters[key] === value) ? renderItems(page.items) : el("p", {}, page.loading ? "正在读取历史摘要…" : "尚未读取历史摘要。"),
    el("div", { className: "action-bar-buttons" },
      el("span", { className: "muted" }, `本页 ${page.items.length} 条 / 匹配 ${page.total} 条 · 读取 ${page.readAt ? formatDate(page.readAt, true) : "尚无"}`),
      actionButton("上一页", { disabled: page.loading || !page.previous.length || !api.configured, onClick: () => load(page.previous.at(-1), "previous") }),
      actionButton("下一页", { disabled: page.loading || !page.next_cursor || !api.configured, onClick: () => load(page.next_cursor, "next") }),
      actionButton("刷新首页", { disabled: page.loading || !api.configured, onClick: () => load("", "refresh") }),
    ),
  )));
  const load = async (cursor, direction, query = page.filters) => {
    const request = historyData.load(name, { cursor, direction, filters: { ...query, ...fixed } });
    draw();
    await request;
    draw();
  };
  draw();
  if ((!page.loaded || Object.entries(fixed).some(([key, value]) => page.filters[key] !== value)) && !page.loading && !page.error && api.configured) void load("", "refresh");
  const container = el("div", { className: "grid", dataset: { liveRegion: `history-${widgetKey}` } }, filters ? el("div", { className: "filter-bar" }, select, search,
    actionButton("查询", { disabled: !api.configured, onClick: () => load("", "refresh", { state: select.value, query: search.value.trim() }) })) : null, host);
  listRefreshers.set(widgetKey, { draw, container });
  return container;
}

function currentDetail(kind, id) {
  const detail = historyData.detail(kind, id);
  if (!isDemo() && detail.value && detail.stale && !detail.loading && !detail.error && api.configured) void historyData.readDetail(kind, id).then(() => renderIfIdle());
  return detail;
}

function renderDetailLoading(kind, id, detail) {
  if (!detail.loading && !detail.error && api.configured) void historyData.readDetail(kind, id).then(() => renderIfIdle());
  return el("div", {}, pageHeader("Record detail", id, "详情单独读取；摘要不包含完整操作，也不能用于批准。"),
    detail.error ? callout("warning", "详情读取失败", detail.error.message) : el("p", {}, "正在读取完整权威记录…"),
    detailRefresh(kind, id));
}

function detailRefresh(kind, id) {
  return actionButton("重新读取完整详情", { disabled: !api.configured || historyData.detail(kind, id).loading, onClick: async () => {
    const draft = document.getElementById("approval-reason")?.value;
    await historyData.readDetail(kind, id);
    renderApp({ focus: false });
    const reason = document.getElementById("approval-reason");
    if (reason && draft !== undefined) reason.value = draft;
  } });
}

function scopeSummary() {
  if (!activeConfiguration) return callout("warning", "修复依赖未配置", "请由可信宿主加载活动修复配置后刷新。");
  return detailList([
    ["目标", el("span", { className: "table-primary" }, activeConfiguration.targetId)],
    ["目标根", copyable(activeConfiguration.targetRoot, "复制目标根")],
    ["允许文件", el("div", { className: "scope-files" }, (activeConfiguration.allowedFiles ?? []).map((file) => tag(file, { mono: true })))],
    ["执行 Harness", el("span", { className: "mono" }, activeConfiguration.executionHarness)],
    ["审批方式", activeConfiguration.reviewer],
    ["审批委托", activeConfiguration.delegation],
    ["有效期", `${activeConfiguration.ttlSeconds} 秒`],
    ["运行限制", `${activeConfiguration.timeoutSeconds} 秒 · 最多 ${activeConfiguration.maxToolCalls} 次工具调用`],
  ]);
}

function renderNewRepair() {
  if (!activeConfiguration) return el("div", {}, pageHeader("New repair", "启动受限修复", "缺少活动修复配置。"), scopeSummary());
  const prompt = el("textarea", {
    id: "repair-prompt",
    className: "textarea",
    maxlength: "8192",
    placeholder: "准确描述希望检查或调整的内容……",
  });
  const byteCounter = el("span", {}, "0 / 8192 字节");
  bindUtf8Limit(prompt, byteCounter, 8192);
  const taskCounter = el("span", {}, "0 / 128 字节");
  const taskId = el("input", {
    id: "repair-task-id",
    className: "field mono",
    value: "",
    maxlength: "128",
  });
  bindUtf8Limit(taskId, taskCounter, 128);
  const form = el(
    "form",
    { dataset: { liveRegion: "repair-request-form" }, on: { submit: (event) => event.preventDefault() } },
    el(
      "div",
      { className: "form-grid" },
      el(
        "div",
        { className: "form-group" },
        el("label", { className: "form-label", htmlFor: "repair-config" }, "外部 repair 配置", tag(isDemo() ? "只读演示" : "服务快照", { demo: isDemo() })),
        el("input", { id: "repair-config", className: "field mono", value: activeConfiguration.repairConfig, disabled: true }),
        el("p", { className: "form-hint" }, "活动配置必须位于 Recuvora 源码与修复目标之外。"),
      ),
      el(
        "div",
        { className: "form-group" },
        el("label", { className: "form-label", htmlFor: "repair-task-id" }, "任务 ID", taskCounter),
        taskId,
        el("p", { className: "form-hint" }, "同一任务 ID 已存在持久操作时不能重放。"),
      ),
      el(
        "div",
        { className: "form-group form-group-wide" },
        el("label", { className: "form-label", htmlFor: "repair-prompt" }, "用户请求", byteCounter),
        prompt,
        el("p", { className: "form-hint" }, "文件、日志和模型内容都是数据，不能扩大此请求的授权范围。"),
      ),
    ),
    el(
      "div",
      { className: "form-footer" },
      previewDisabledReason(),
      operationAction("启动受限修复", { variant: "primary", iconName: "play", permission: "repair.run", available: configuredCapability("text-repair"), key: "repair.run", path: "/api/v1/repairs/runs", body: () => {
        if (!form.reportValidity() || !prompt.value.trim() || !taskId.value.trim()) throw new Error("请输入有效任务 ID 和用户请求。");
        const context = diagnosisContext;
        if (context && context.target !== activeConfiguration?.targetId) throw new Error("故障目标与当前修复配置不同，请先配置对应目标或清除诊断上下文。");
        return { prompt: prompt.value, task_id: taskId.value.trim(), ...(context ? { incident_id: context.incidentId, incident_revision: context.revision } : {}) };
      } }),
    ),
  );
  return el(
    "div",
    {},
    pageHeader(
      "New repair",
      "启动受限修复",
      "执行 Harness 可读取列出的文件并提出准确全文替换；是否自动执行取决于可信政策和具体审批结果。",
      [linkButton("返回任务", "#/repairs", { iconName: "arrow" })],
    ),
    diagnosisCard(),
    el(
      "div",
      { className: "stepper", "aria-label": "新建修复步骤" },
      ["选择可信配置", "确认操作范围", "提交准确请求"].map((label, index) =>
        el("div", { className: `stepper-item ${index === 2 ? "stepper-item-active" : ""}` }, el("span", { className: "stepper-number" }, index + 1), label),
      ),
    ),
    callout(
      "warning",
      "提交后可能执行真实文件写入",
      "若政策指定的 Harness 评审批准，当前流程可在本轮执行获准替换；人工审批时会停止在待处理状态。请先核对目标、完整允许范围和准确请求。",
      { iconName: "warning" },
    ),
    el(
      "div",
      { className: "content-grid section" },
      card("请求", "当前页面检查 UTF-8 字节；可信运行时仍会重新校验", form),
      card("可信范围", "来自 repair 配置的只读快照", scopeSummary(), { headerAction: statusChip(capabilityStates.limited) }),
    ),
  );
}

function repairTimeline(repair) {
  const hasOperations = repair.operationCount > 0;
  const unknown = repair.status === "unknown";
  const waiting = repair.status === "waiting_human";
  const completed = repair.status === "completed";
  const failed = repair.status === "failed" || repair.status === "blocked";
  const steps = [
    { title: "请求进入本轮流程", detail: `任务 ${repair.id}`, state: "complete" },
    { title: "Harness 执行会话", detail: repair.threadId ? `线程 ${repair.threadId}` : "没有已知线程", state: failed && !hasOperations ? "current" : "complete" },
    { title: "具体操作审批", detail: hasOperations ? `${repair.operationCount} 项持久审批记录` : "没有创建审批请求", state: waiting ? "current" : hasOperations ? "complete" : "idle" },
    { title: "文件执行", detail: unknown ? "回执不可确认" : completed ? "执行事实请核对审批记录" : waiting ? "尚未执行" : "没有可报告的执行结果", state: unknown ? "unknown" : completed && hasOperations ? "complete" : completed || waiting ? "idle" : "current" },
    { title: "业务恢复验证", detail: "当前版本未实现", state: "idle" },
  ];
  return el(
    "ol",
    { className: "timeline" },
    steps.map((step) =>
      el(
        "li",
        { className: "timeline-item" },
        el(
          "span",
          { className: `timeline-node ${step.state === "complete" ? "timeline-node-complete" : step.state === "current" ? "timeline-node-current" : step.state === "unknown" ? "timeline-node-unknown" : ""}` },
          icon(step.state === "complete" ? "check" : step.state === "unknown" ? "warning" : "clock", 13),
        ),
        el("div", { className: "timeline-copy" }, el("div", { className: "timeline-title" }, step.title), el("div", { className: "timeline-detail" }, step.detail)),
      ),
    ),
  );
}

function renderRepairDetail(id) {
  const detail = currentDetail("repairs", id);
  if (!isDemo() && !detail.value) return renderDetailLoading("repairs", id, detail);
  const repair = isDemo() ? repairs.find((item) => item.id === id) : detail.value;
  if (!repair) return renderEntityNotFound("修复任务", id, "#/repairs");
  const relatedApprovals = isDemo() ? approvals.filter((item) => repair.operationIds.includes(item.requestId)) : historyData.page("related-approvals").items;
  const meta = repairStates[repair.status] ?? repairStates.failed;
  const alert = repair.status === "unknown"
    ? callout("unknown", "结果未知：不要重新执行", "写入可能已经发生。应先读取并核验准确目标；同一实际目标的新操作必须保持阻断。", { role: "alert" })
    : repair.status === "completed"
      ? callout("info", "本轮流程已结束", "文件执行事实请核对审批记录；流程结束可能没有产生文件动作，业务恢复未验证。", { iconName: "check" })
      : callout(meta.tone === "warning" ? "warning" : "info", meta.label, meta.description, { iconName: repair.status === "waiting_human" ? "clock" : "info" });
  return el(
    "div",
    {},
    pageHeader(
      "Repair detail",
      repair.id,
      repair.summary,
      [linkButton("返回任务", "#/repairs", { iconName: "arrow" }), statusChip(meta)],
    ),
    alert,
    repair.sourceIncident ? el("section", { className: "section", dataset: { patchKey: "repair-source-incident" } }, card("来源故障", "持久来源关联，不代表故障已解除或修复成功。", detailList([
      ["故障", linkButton(repair.sourceIncident.summary ?? repair.sourceIncident.id, `#/monitoring/incidents/${safeSegment(repair.sourceIncident.id)}`)],
      ["目标", repair.sourceIncident.target_id], ["提交时证据版本", String(repair.sourceIncident.revision)], ["关联时间", formatDate(repair.sourceIncident.captured_at, true)],
    ]))) : null,
    !isDemo() ? detailRefresh("repairs", id) : null,
    !isDemo() && detail.stale ? callout("warning", "详情需要更新", detail.error?.message ?? "正在重新读取完整详情。") : null,
    el(
      "div",
      { className: "content-grid section" },
      el(
        "div",
        { className: "grid" },
        card("流程视图", "权威操作事实以审批记录为准", repairTimeline(repair)),
        card("Harness 最终回复", "外部文本不会按 Markdown 或 HTML 解释", textFrame("Final response", repair.finalResponse)),
      ),
      el(
        "div",
        { className: "grid" },
        card("任务上下文", isDemo() ? "本轮演示数据" : "服务返回的本轮上下文", detailList([
          ["任务 ID", copyable(repair.id, "复制任务 ID")],
          ["目标", repair.target],
          ["Harness", el("span", { className: "mono" }, repair.harnessId)],
          ["线程", copyable(repair.threadId, "复制线程 ID")],
          ["会话", copyable(repair.sessionId, "复制会话 ID")],
          ["更新时间", formatDate(repair.updatedAt, true)],
          ["业务验证", businessVerificationChip()],
          ["自动重试", statusChip({ label: "关闭", tone: "neutral" })],
        ])),
        card("操作记录", `${repair.operationCount} 项持久记录`, isDemo() ? approvalTable(relatedApprovals) : renderHistoryList("related-approvals", approvalTable, null, { task_id: id })),
      ),
    ),
  );
}

function approvalTable(items) {
  if (!items.length) {
    return el("div", { className: "card empty-state" }, el("div", {}, el("span", { className: "empty-icon" }, icon("search", 20)), el("h2", {}, "没有匹配的审批记录"), el("p", {}, "调整筛选条件。界面不会把提供方错误当成空结果。")));
  }
  return table(
    ["审批请求", "目标 / 文件", "状态", "评审来源", "Revision", "更新时间", "到期时间"],
    items.map((approval) => [
      el(
        "div",
        {},
        el("a", { className: "table-link", href: `#/approvals/${safeSegment(approval.requestId)}` }, approval.requestId),
        el("span", { className: "table-secondary" }, approval.taskId),
      ),
      el("div", {}, el("span", { className: "table-primary" }, approval.target), el("span", { className: "table-secondary mono" }, approval.path)),
      statusChip(approvalStates[approval.state]),
      approval.assessment ? `${approval.assessment.source === "human" ? "人工" : "Harness"} · ${approval.assessment.reviewer}` : approval.detail === false ? "详情按需读取" : "尚无评审",
      el("span", { className: "mono numeric" }, `r${approval.revision}`),
      el("span", { className: "numeric" }, formatDate(approval.updatedAt, true)),
      el("span", { className: "numeric" }, formatDate(approval.expiresAt, true)),
    ]),
    "操作审批记录",
  );
}

function renderApprovals() {
  return el("div", {}, pageHeader("Authorization", "操作审批", "先读取需处理请求；可筛选全部历史并分页。完整文件差异与政策只在详情页读取。"),
    callout("unknown", "Unknown 必须先核验", "结果未知表示操作可能已经发生；不会重复执行。"),
    isDemo() ? approvalTable(approvals) : renderHistoryList("approvals", approvalTable, [["attention", "需处理"], ["all", "全部"], ["waiting_human", "待人工"], ["approved", "批准待执行"], ["executing", "正在执行"], ["pending", "等待评审"], ["unknown", "结果未知"], ["executed", "已执行"], ["denied", "已拒绝"]]));
}

function approvalCallout(approval) {
  if (approval.state === "unknown") {
    return callout("unknown", "结果未知：写入可能已经发生", "请选择“读取并核验”确认当前内容，切勿重新执行。核验不重复写入，但真实运行时会更新权威记录。", { role: "alert" });
  }
  if (approval.state === "waiting_human") {
    return callout("warning", "需要人工决定", "请逐项核对用户请求、完整文本差异和策略范围。批准与执行是两个独立步骤。", { iconName: "clock" });
  }
  if (approval.state === "executed") {
    return callout("success", "获准文件操作已执行", "读回内容与批准文本一致；这不是目标业务已经恢复的证据。", { iconName: "check" });
  }
  if (approval.state === "denied") {
    return callout("info", "操作已拒绝", "该审批记录为终态，没有执行文件写入，也不能通过旧请求再次批准。", { iconName: "shield" });
  }
  return callout("info", approvalStates[approval.state]?.label ?? "审批状态", approval.note ?? "查看当前权威记录。", { iconName: "info" });
}

function approvalActionBar(approval) {
  const reason = el("textarea", { id: "approval-reason", className: "textarea", maxlength: "1024", placeholder: "填写本次决定或核验的具体理由", "aria-label": "决定理由" });
  const allowed = !isDemo() && historyData.detail("approvals", approval.requestId).stale ? [] : approval.allowedActions ?? [];
  const catalog = [
    ["approve", "批准", "primary", "check"],
    ["deny", "拒绝", "danger", "close"],
    ["revoke", "撤销", "", "stop"],
    ["apply", "执行已批准替换", "danger", "play"],
    ["reconcile", "读取并核验当前内容", "primary", "eye"],
  ];
  const actions = catalog.filter(([action]) => isDemo() || allowed.includes(action)).map(([action, label, variant, iconName]) => operationAction(label, {
    permission: action === "apply" ? "approval.apply" : action === "reconcile" ? "approval.reconcile" : "approval.decide", available: allowed.includes(action) && Number.isSafeInteger(approval.revision),
    key: `approval:${approval.requestId}`, path: `/api/v1/approvals/${safeSegment(approval.requestId)}/${action}`, variant, iconName,
    body: () => {
      const mountedReason = document.getElementById("approval-reason") ?? reason;
      if (!mountedReason.value.trim()) throw new Error("请填写本次决定的具体理由。");
      return { revision: approval.revision, reason: mountedReason.value.trim() };
    },
  }));
  return el("section", { className: "section", dataset: { patchKey: `decision-${approval.requestId}` } },
    ["waiting_human", "approved", "unknown", "pending"].includes(approval.state) || actions.length ? el("div", { dataset: { patchKey: "approval-reason-card" } }, card("决定理由", "说明本次批准、拒绝、执行或核验的依据。", reason)) : null,
    el("div", { className: "action-bar approval-action-bar" },
      el("div", { className: "action-bar-copy" }, `${approvalStates[approval.state]?.label ?? "操作审批"} · ${approval.requestId} · revision ${approval.revision}`),
      actions.length ? el("div", { className: "action-bar-buttons" }, actions) : el("span", { className: "muted" }, "当前记录只读；没有可用动作。"),
    ),
  );
}

function renderApprovalDetail(id) {
  const detail = currentDetail("approvals", id);
  if (!isDemo() && !detail.value) return renderDetailLoading("approvals", id, detail);
  const approval = isDemo() ? approvals.find((item) => item.requestId === id) : detail.value;
  if (!approval) return renderEntityNotFound("审批请求", id, "#/approvals");
  const meta = approvalStates[approval.state] ?? approvalStates.failed;
  return el(
    "div",
    {},
    pageHeader(
      "Approval detail",
      `${approval.target} · ${approval.path}`,
      `审查准确文件替换 · 到期 ${formatDate(approval.expiresAt, true)}`,
      [linkButton("返回审批", "#/approvals", { iconName: "arrow" }), linkButton("查看修复任务", `#/repairs/${safeSegment(approval.taskId)}`), statusChip(meta)],
    ),
    approvalCallout(approval),
    !isDemo() ? detailRefresh("approvals", id) : null,
    !isDemo() && detail.stale ? callout("warning", "详情需要更新", detail.error?.message ?? "正在重新读取当前 revision；更新前无法提交决定。") : null,
    el(
      "div",
      { className: "content-grid section", dataset: { patchKey: `approval-content-${id}` } },
      el(
        "div",
        { className: "grid" },
        card(
          "准确文件替换",
          "文件内容是不可信数据，只按纯文本显示",
          el(
            "div",
            { className: "grid" },
            detailList([
              ["目标", approval.target],
              ["目标根", copyable(approval.targetRoot, "复制目标根")],
              ["文件", copyable(approval.path, "复制文件路径")],
              ["动作", tag(approval.actionKind, { mono: true })],
            ]),
            diffViewer(approval, isDemo()),
          ),
          { headerAction: tag("完整替换", { mono: true }) },
        ),
        card("原始用户请求", "权限只能来自可信政策与明确请求", textFrame("User request", approval.userRequest)),
        approval.assessment
          ? card("评审结论", `${approval.assessment.source === "human" ? "人工操作员" : "独立 Hidden Harness 会话"}`, detailList([
              ["决定", statusChip({ label: approval.assessment.decision === "approve" ? "批准" : approval.assessment.decision === "deny" ? "拒绝" : "升级人工", tone: approval.assessment.decision === "approve" ? "success" : approval.assessment.decision === "deny" ? "danger" : "warning" })],
              ["评审者", approval.assessment.reviewer],
              ["会话", copyable(approval.assessment.sessionId, "复制评审会话 ID")],
              ["理由", approval.assessment.reason],
            ]))
          : card("评审结论", "尚无结构化评审", el("span", { className: "muted" }, "请求仍等待评审。")),
      ),
      el(
        "div",
        { className: "grid" },
        card("授权边界", "请求创建时的完整策略快照", detailList([
          ["策略", `${approval.policy.id} · v${approval.policy.version}`],
          ["评审方式", approval.policy.reviewer],
          ["允许目标", el("div", { className: "scope-files" }, approval.policy.allowedTargets.map((value) => tag(value)))],
          ["允许动作", el("div", { className: "scope-files" }, approval.policy.allowedActions.map((value) => tag(value, { mono: true })))],
          ["委托", approval.policy.delegation],
          ["TTL", `${approval.policy.ttlSeconds} 秒`],
        ]), { headerAction: statusChip(capabilityStates.limited) }),
        el("details", { dataset: { patchKey: "approval-metadata" } }, el("summary", { className: "section-link" }, "记录与执行关联"), card("记录元数据", "权威状态必须由服务端重新读取", detailList([
          ["Request ID", copyable(approval.requestId, "复制请求 ID")],
          ["Revision", el("span", { className: "mono" }, approval.revision)],
          ["Task revision", el("span", { className: "mono" }, approval.taskRevision)],
          ["创建", formatDate(approval.createdAt, true)],
          ["更新", formatDate(approval.updatedAt, true)],
          ["到期", formatDate(approval.expiresAt, true)],
          ["备注", approval.note ?? "—"],
        ])),
        card("执行关联", "来源标识由宿主关联，不接受模型填报", detailList([
          ["Harness", approval.executionContext.harnessId],
          ["Thread", copyable(approval.executionContext.threadId, "复制线程 ID")],
          ["Turn", copyable(approval.executionContext.turnId, "复制轮次 ID")],
          ["Call", copyable(approval.executionContext.callId, "复制调用 ID")],
          ["文件读回", approval.receipt?.contentVerified ? statusChip({ label: "内容一致", tone: "success" }) : "无确定回执"],
          ["业务恢复", businessVerificationChip()],
        ]))),
        jsonDetails("查看完整规范化记录 JSON", approval),
      ),
    ),
    approvalActionBar(approval),
  );
}

function harnessCard(harness) {
  const availability = harnessAvailability[harness.availability] ?? harnessAvailability.unavailable;
  return el(
    "article",
    { className: "card card-interactive harness-card" },
    el(
      "div",
      { className: "harness-card-top" },
      el(
        "div",
        {},
        el("h3", {}, harness.id),
        el("div", { className: "harness-adapter" }, `${harness.adapter} · ${harness.address}`),
      ),
      statusChip(availability),
    ),
    el(
      "div",
      { className: "harness-stat-row" },
      el("div", { className: "harness-stat" }, el("div", { className: "harness-stat-label" }, "认证"), el("div", { className: "harness-stat-value" }, harness.authentication ?? "not_checked")),
      el("div", { className: "harness-stat" }, el("div", { className: "harness-stat-label" }, "运行状态"), el("div", { className: "harness-stat-value" }, harness.runtimeStatus ?? "not_probed")),
      el("div", { className: "harness-stat" }, el("div", { className: "harness-stat-label" }, "工作区根"), el("div", { className: "harness-stat-value numeric" }, `${harness.workspaceRoots.length} 个`)),
      el("div", { className: "harness-stat" }, el("div", { className: "harness-stat-label" }, "原生项目"), el("div", { className: "harness-stat-value numeric" }, isDemo() ? `${harness.projects?.length ?? 0} 个演示` : "需显式探测")),
    ),
    harness.reason ? el("p", { className: "capability-limitation" }, harness.reason) : null,
    el(
      "div",
      { className: "harness-card-footer" },
      el("div", { className: "scope-files" }, harness.isDefault ? tag("默认") : null, harness.enabled ? tag("已启用") : tag("已停用")),
      el("a", { className: "section-link", href: `#/harnesses/${safeSegment(harness.id)}` }, "查看详情", icon("arrow", 13)),
    ),
  );
}

function renderHarnesses() {
  return el(
    "div",
    {},
    pageHeader(
      "AI harnesses",
      "AI 会话与实例",
      "管理已经配置的 AI 执行与评审实例。配置成功不代表已登录、在线或通过运行探测。",
      [actionButton("刷新服务快照", { iconName: "activity", disabled: !api.configured, onClick: startPolling })],
    ),
    diagnosisCard(),
    callout("info", "配置、认证与运行状态彼此独立", "列表读取配置时不会启动 app-server，也不会检查认证。失败实例必须显示原因，不能伪装成空项目列表。", { iconName: "harness" }),
    section(
      "已配置实例",
      `${harnesses.length} 个${isDemo() ? "演示" : "服务配置"}实例 · 可用性由可信宿主及提供方状态决定`,
      el("div", { className: "grid grid-3" }, harnesses.map(harnessCard)),
    ),
    section(
      "调用语义",
      "所有外部提交默认不自动重试。",
      el(
        "div",
        { className: "grid grid-3" },
        card("显式实例选择", "失败时不静默改派", el("p", { className: "muted" }, "省略 Harness 只使用明确配置的默认实例；不存在默认项时关闭失败。")),
        card("可见性不是网络边界", "Client / Hidden", el("p", { className: "muted" }, "Hidden 只影响客户端历史可见性，不代表离线，也不改变数据发送路径。")),
        card("Unknown 保留关联", "线程或项目可能已创建", el("p", { className: "muted" }, "保留已知 thread ID、project ID 或幂等键，先核验再继续。")),
      ),
    ),
  );
}

function projectsTable(harness) {
  if (harness.availability === "unavailable") {
    return callout("warning", "项目探测不可用", harness.reason ?? "实例无法装配；不能将此错误显示为空项目列表。", { iconName: "warning" });
  }
  const view = isDemo() ? { projects: harness.projects ?? [], status: "loaded" } : projectViews.get(harness.id);
  if (!view) return callout("info", "尚未探测原生项目", "请显式选择工作区并读取项目；未探测不等于空项目列表。");
  if (view.status === "loading") return callout("info", "正在探测项目", "使用选定实例和工作区，读取有超时上限。");
  if (view.error) return callout("warning", "项目探测失败", view.error);
  if (!view.projects.length) {
    return el(
      "div",
      { className: "empty-state" },
      el("div", {}, el("span", { className: "empty-icon" }, icon("folder", 19)), el("h2", {}, "探测完成，无原生项目"), el("p", {}, isDemo() ? "演示样例" : "选定实例明确返回空列表。")),
    );
  }
  return table(
    ["原生项目 ID", "名称", "根目录", "客户端归组"],
    view.projects.map((project) => [
      copyable(project.id, "复制项目 ID"),
      el("span", { className: "table-primary" }, project.name),
      el("div", { className: "scope-files" }, project.roots.map((root) => tag(root, { mono: true }))),
      statusChip({ label: "未核验", tone: "warning" }),
    ]),
    `${harness.id} 的原生项目列表`,
  );
}

function renderHarnessDetail(id) {
  const harness = harnesses.find((item) => item.id === id);
  if (!harness) return renderEntityNotFound("Harness", id, "#/harnesses");
  const availability = harnessAvailability[harness.availability] ?? harnessAvailability.unavailable;
  const workspace = el("select", { className: "select", "aria-label": "项目探测工作区" }, harness.workspaceRoots.map((root) => el("option", { value: root }, root)));
  const projectHost = el("div", {}, projectsTable(harness));
  return el(
    "div",
    {},
    pageHeader(
      "Harness detail",
      harness.id,
      `${harness.adapter} · ${harness.address}`,
      [linkButton("返回 Harness", "#/harnesses", { iconName: "arrow" }), statusChip(availability)],
    ),
    harness.availability === "unavailable"
      ? callout("warning", "实例无法装配", harness.reason ?? "检查外部配置和受信任可执行程序。", { role: "alert" })
      : callout("info", "已配置，不等于在线", "认证尚未检查，app-server 运行状态也尚未探测。页面不会显示绿色在线状态。", { iconName: "harness" }),
    el(
      "div",
      { className: "content-grid section" },
      el(
        "div",
        { className: "grid" },
        card(
          "原生项目",
          "项目 ID 仅在当前 Harness 内有效",
          el("div", { className: "grid" }, workspace, actionButton("探测原生项目", { disabled: !can("harness.projects") || harness.availability !== "configured", iconName: "folder", onClick: () => discoverProjects(harness, workspace.value, () => projectHost.replaceChildren(projectsTable(harness))) }), projectHost),
          {
            headerAction: disabledAction("登记已有项目", { iconName: "plus", title: "当前 HTTP 服务未提供原生项目创建能力" }),
            footer: el("span", { className: "muted" }, "登记只关联已存在且位于允许根中的目录，不会创建文件夹。"),
          },
        ),
        card(
          "文本轮次",
          "显式选择可见性和原生项目策略",
          el(
            "div",
            {},
            el("p", { className: "muted" }, "Client 会话可能进入客户端历史；Hidden 会话不请求原生项目，但两者都不代表离线。"),
            el("div", { className: "section" }, linkButton("配置文本轮次", `#/harnesses/${safeSegment(harness.id)}/run`, { variant: "primary", iconName: "terminal" })),
          ),
        ),
      ),
      el(
        "div",
        { className: "grid" },
        card("实例配置", isDemo() ? "只读演示" : "服务提供的只读配置", detailList([
          ["ID", copyable(harness.id, "复制 Harness ID")],
          ["Adapter", el("span", { className: "mono" }, harness.adapter)],
          ["Address", el("span", { className: "mono" }, harness.address)],
          ["启用", harness.enabled ? "是" : "否"],
          ["默认", harness.isDefault ? "是" : "否"],
          ["认证", harness.authentication ?? "not_checked"],
          ["运行", harness.runtimeStatus ?? "not_probed"],
        ])),
        card("允许工作区", `${harness.workspaceRoots.length} 个规范化根`, el("div", { className: "scope-files" }, harness.workspaceRoots.map((root) => tag(root, { mono: true })) )),
        el("p", { className: "muted" }, "项目探测与文本调用的结果不能替代独立客户端归组核验。"),
      ),
    ),
  );
}

async function discoverProjects(harness, workspace, update) {
  if (!can("harness.projects") || projectViews.get(harness.id)?.status === "loading") return;
  pageController?.abort();
  const controller = new AbortController();
  pageController = controller;
  const generation = connectionGeneration;
  projectViews.set(harness.id, { workspace, status: "loading", projects: [] });
  update();
  try {
    const result = await api.projects(harness.id, { cwd: workspace, signal: controller.signal });
    if (!Array.isArray(result.projects)) throw new ApiError("invalid_response", "项目探测响应没有项目列表。");
    if (generation === connectionGeneration) projectViews.set(harness.id, { workspace, status: "loaded", projects: result.projects });
  } catch (error) {
    if (generation === connectionGeneration) projectViews.set(harness.id, { workspace, status: "failed", projects: [], error: error.message });
  } finally { if (generation === connectionGeneration) update(); }
}

function renderHarnessRun(id) {
  const harness = harnesses.find((item) => item.id === id);
  if (!harness) return renderEntityNotFound("Harness", id, "#/harnesses");
  const prompt = el("textarea", { id: "harness-prompt", className: "textarea", maxlength: "65536", placeholder: "输入单轮文本请求……", value: diagnosisContext ? `调查故障 ${diagnosisContext.incidentId}：${diagnosisContext.summary}\n目标：${diagnosisContext.target}\n证据：${JSON.stringify(diagnosisContext.evidence)}\n请先诊断原因，仅提出目标授权范围内的操作建议。` : "" });
  const counter = el("span", {}, "0 / 65536 字节");
  bindUtf8Limit(prompt, counter, 65536);
  const workspace = el("select", { id: "run-cwd", className: "select mono" }, harness.workspaceRoots.map((root) => el("option", { value: root }, root)));
  const visibility = el("select", { id: "visibility", className: "select", required: true }, el("option", { value: "" }, "请选择可见性"), el("option", { value: "client" }, "Client · 客户端可见"), el("option", { value: "hidden" }, "Hidden · 客户端隐藏"));
  const project = el(
    "select",
    { id: "native-project", className: "select", required: true },
    el("option", { value: "" }, "请选择原生项目策略"),
    el("option", { value: "none" }, "不请求原生项目"),
  );
  const projectStatus = el("div", {});
  const updateProjects = () => {
    const view = isDemo() ? { projects: harness.projects ?? [] } : projectViews.get(harness.id);
    const previous = project.value;
    project.replaceChildren(el("option", { value: "" }, "请选择原生项目策略"), el("option", { value: "none" }, "不请求原生项目"),
      (view?.workspace === workspace.value || isDemo() ? view.projects : []).map((item) => el("option", { value: item.id }, `${item.name} · ${item.id}`)));
    if ([...project.options].some((option) => option.value === previous)) project.value = previous;
    projectStatus.replaceChildren(projectsTable(harness));
  };
  workspace.addEventListener("change", () => { project.value = ""; updateProjects(); });
  updateProjects();
  visibility.addEventListener("change", () => {
    if (visibility.value === "hidden") project.value = "none";
    project.disabled = visibility.value === "hidden";
  });
  const form = el(
    "form",
    { dataset: { liveRegion: `harness-request-form-${id}` }, on: { submit: (event) => event.preventDefault() } },
    el(
      "div",
      { className: "form-grid" },
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "run-harness" }, "Harness"), el("input", { id: "run-harness", className: "field mono", value: harness.id, disabled: true })),
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "run-cwd" }, "允许工作区 / 工作目录"), workspace, el("p", { className: "form-hint" }, "仅使用服务提供的 workspace ID 或本机允许根，实际路径由提供方映射。")),
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "visibility" }, "会话可见性"), visibility, el("p", { className: "form-hint" }, "Hidden 不是离线，也不改变数据发送路径。")),
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "native-project" }, "Harness 原生项目"), project, actionButton("探测此工作区的项目", { disabled: !can("harness.projects") || harness.availability !== "configured", onClick: () => discoverProjects(harness, workspace.value, updateProjects) }), el("p", { className: "form-hint" }, "无原生项目不保证客户端不会按 cwd 归组。")),
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "run-model" }, "模型（可选）"), el("input", { id: "run-model", className: "field", placeholder: "使用 Harness 默认模型" })),
      el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "run-timeout" }, "总超时"), el("select", { id: "run-timeout", className: "select" }, el("option", { value: "180" }, "180 秒（默认）"), el("option", { value: "300" }, "300 秒"), el("option", { value: "600" }, "600 秒"))),
      el("div", { className: "form-group form-group-wide" }, el("label", { className: "form-label", htmlFor: "harness-prompt" }, "Prompt", counter), prompt),
    ),
    el("div", { className: "form-footer" }, previewDisabledReason(), operationAction("发起文本轮次", { variant: "primary", iconName: "play", permission: "harness.run", available: harness.enabled && harness.availability === "configured" && configuredCapability("harness"), key: `harness:${id}`, path: `/api/v1/harness/${safeSegment(id)}/runs`, body: () => {
      if (!form.reportValidity() || !prompt.value.trim() || !visibility.value || !project.value || !workspace.value) throw new Error("请输入请求，并显式选择工作区、可见性和项目策略。");
      const view = projectViews.get(id);
      if (project.value !== "none" && (view?.workspace !== workspace.value || !view.projects.some((item) => item.id === project.value))) throw new Error("选中项目尚未由此实例在当前工作区探测确认。");
      return { workspace_id: workspace.value, prompt: prompt.value, visibility: visibility.value, project_id: project.value === "none" ? null : project.value, model: form.querySelector("#run-model").value.trim() || null, timeout_secs: Number(form.querySelector("#run-timeout").value) };
    } })),
  );
  return el(
    "div",
    {},
    pageHeader("Harness run", "配置文本轮次", `所选实例：${harness.id}`, [linkButton("返回实例", `#/harnesses/${safeSegment(harness.id)}`, { iconName: "arrow" })]),
    diagnosisCard(),
    callout("warning", "外部提交不自动重试", "取消或超时发生在提交后时，会话结果可能未知。应保留已知线程和项目关联后核验。", { iconName: "warning" }),
    section("调用参数", "项目探测为独立读取；文本提交使用选定超时且不自动重试。", card(null, null, form)),
    section("项目探测结果", "失败不会显示为空列表。", projectStatus),
  );
}

function renderSimulation() {
  const successes = simulationTasks.filter((item) => item.state === "succeeded").length;
  const unknown = simulationTasks.filter((item) => item.state === "unknown").length;
  const taskId = el("input", { id: "simulation-task", className: "field mono", placeholder: "唯一的模拟任务 ID", maxlength: "128" });
  const target = el("input", { id: "simulation-target", className: "field mono", placeholder: "模拟目标标识", maxlength: "128" });
  const scenario = el("select", { id: "simulation-scenario", className: "select" }, ["succeed", "fail", "hang", "exit", "verification_failed"].map((value) => el("option", { value }, value)));
  const timeout = el("input", { id: "simulation-timeout", className: "field", type: "number", min: "100", max: "60000", value: "2000" });
  return el(
    "div",
    {},
    pageHeader(
      "Safe simulation",
      "模拟实验室",
      "封闭、无副作用的任务状态原型，与真实 repair、授权和文件操作完全隔离。",
      [operationAction("运行模拟场景", { variant: "primary", iconName: "play", permission: "simulation.run", available: configuredCapability("simulation"), key: "simulation.run", path: "/api/v1/simulations", body: () => {
        if (!taskId.value.trim() || !target.value.trim() || !timeout.reportValidity()) throw new Error("请填写模拟任务、目标和有效超时。");
        return { task_id: taskId.value.trim(), target: target.value.trim(), scenario: scenario.value, timeout_ms: Number(timeout.value) };
      } })],
    ),
    el(
      "div",
      { className: "simulation-banner" },
      el("div", { className: "eyebrow" }, "Simulation only"),
      el("h2", {}, "不会调用 Harness、命令或真实目标操作"),
      el("p", {}, isDemo() ? "以下结果是演示夹具，不代表真实能力。模拟授权不能赋予任何真实权限。" : "下方状态来自宿主模拟引擎；模拟结果不代表真实恢复能力，模拟授权不能赋予任何真实权限。"),
    ),
    section(
      "模拟结果",
      "计数来自最近初始化快照的首批记录；下方可独立翻阅历史，模拟与真实任务分开记录。",
      el(
        "div",
        { className: "grid grid-4" },
        metricCard("首批记录", simulationTasks.length, "初始化快照的模拟摘要", "simulation", "primary"),
        metricCard("模拟成功", successes, "只说明预期状态迁移", "check", "success"),
        metricCard("模拟未知", unknown, "回执不确定时保守停止", "warning", "unknown"),
        metricCard("自动重试", "关闭", "Unknown 持续阻断同目标", "lock", "warning"),
      ),
    ),
    section(
      "任务快照",
      isDemo() ? "Revision 和状态均为演示数据。" : "Revision 和状态均由宿主模拟引擎返回。",
      (isDemo() ? (render) => render(simulationTasks) : (render) => renderHistoryList("simulation_tasks", render))((items) => table(
        ["任务", "目标", "场景", "状态", "Revision", "用时"],
        items.map((task) => [
          el("span", { className: "table-link" }, task.taskId ?? task.id),
          el("span", { className: "mono" }, task.target),
          el("span", { className: "mono" }, task.scenario),
          statusChip(simulationStates[task.state]),
          el("span", { className: "mono numeric" }, task.revision === null || task.revision === undefined ? "—" : `r${task.revision}`),
          el("span", { className: "numeric" }, task.duration),
        ]),
        "固定模拟任务结果",
      )),
    ),
    section(
      "运行参数",
      "服务管理模拟数据目录；此处只提交受限场景，不接受命令或文件路径。",
      card(null, null, el(
        "div",
        { className: "form-grid" },
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "simulation-task" }, "任务 ID"), taskId),
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "simulation-target" }, "模拟目标"), target),
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "simulation-scenario" }, "受限场景"), scenario),
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "simulation-timeout" }, "超时（毫秒）"), timeout),
      )),
    ),
  );
}

function capabilityCard(item) {
  const isPlanned = ["planned", "not_implemented"].includes(item.state);
  return el(
    "a",
    {
      className: `card card-interactive capability-card ${isPlanned ? "capability-card-planned" : ""}`,
      href: `#/capabilities/${safeSegment(item.id)}`,
      dataset: { patchKey: `capability:${item.id}` },
    },
    el(
      "div",
      { className: "capability-card-top" },
      el("span", { className: "capability-mark" }, icon(item.id === "harness" ? "harness" : item.id === "simulation" ? "simulation" : item.id === "text-repair" ? "repair" : "capability", 17)),
      statusChip(capabilityStates[item.state]),
    ),
    el("h3", {}, item.name),
    !isPlanned ? tag(({ available: "运行可用", configured: "已配置", unconfigured: "未配置", unavailable: "运行不可用" })[item.availability] ?? "运行状态未报告") : null,
    el("p", { className: "capability-summary" }, item.summary),
    el("div", { className: "capability-limitation" }, item.limitation),
  );
}

function renderCapabilities() {
  const groups = [...new Set(["核心", "当前能力", "运行时能力", "正在建设", "规划能力", ...capabilities.map((item) => item.group)])];
  const filterOptions = [
    ["all", "全部"],
    ["available", "已实现 / 受限"],
    ["building", "开发中"],
    ["planned", "规划 / 未实现"],
  ];
  const visible = capabilities.filter((item) => {
      if (viewState.capabilityAvailability !== "all" && item.availability !== viewState.capabilityAvailability) return false;
      if (viewState.capabilityFilter === "all") return true;
      if (viewState.capabilityFilter === "available") return ["implemented", "prototype", "limited"].includes(item.state);
      if (viewState.capabilityFilter === "building") return item.state === "in_progress";
      return ["planned", "not_implemented"].includes(item.state);
  });
  const host = el("div", { dataset: { patchKey: "capability-results" } }, groups.map((group) => {
      const items = visible.filter((item) => item.group === group);
      if (!items.length) return null;
      return section(group, group === "规划能力" ? "当前没有可运行实现，也不会生成虚构状态。" : null, el("div", { className: "capability-grid" }, items.map(capabilityCard)));
    }).filter(Boolean));
  const buttons = filterOptions.map(([value, label]) => el("button", {
    className: "filter-button",
    type: "button",
    "aria-pressed": viewState.capabilityFilter === value,
    on: { click: (event) => {
      viewState.capabilityFilter = value;
      renderIfIdle();
      event.currentTarget.focus();
    } },
  }, label));
  const availability = el("select", { id: "capability-availability", className: "select", value: viewState.capabilityAvailability, on: { change: (event) => { viewState.capabilityAvailability = event.currentTarget.value; renderIfIdle(); } } },
    [["all", "全部运行状态"], ["available", "运行可用"], ["configured", "已配置"], ["unconfigured", "未配置"], ["unavailable", "运行不可用"]].map(([value, label]) => el("option", { value, selected: value === viewState.capabilityAvailability }, label)));
  return el(
    "div",
    {},
    pageHeader("Capability registry", "能力中心", "准确区分已实现、受限、原型、正在建设、规划中和未实现；目录存在不代表能力可运行。"),
    callout("info", "页面归属与提供方分别管理", "官方标准页面保留在同一控制台。缺少依赖时显示未配置，不会使用演示数据补齐。扩展只能在已登记契约内调用。", { iconName: "capability" }),
    el("div", { className: "filter-bar section", dataset: { patchKey: "capability-filters" } }, el("div", { className: "filter-tabs", role: "group", "aria-label": "实现阶段筛选" }, buttons), el("label", { htmlFor: "capability-availability" }, "运行状态"), availability),
    host,
    !isDemo() ? section("外部节点与契约扩展", "只有成功连接的提供方可用；配置条目存在不表示已接通。", extensionStatuses.length ? table(["实例", "类型", "可用性", "原因"], extensionStatuses.map((item) => [
      el("span", { className: "mono" }, item.id), el("span", {}, item.kind), statusChip({ label: item.available ? "已连接" : "不可用", tone: item.available ? "success" : "warning" }), el("span", {}, item.error ?? "—"),
    ]), "外部扩展连接状态") : callout("info", "尚无扩展连接状态", "请由可信宿主加载扩展配置；本页不会安装或自动启用提供方。")) : null,
  );
}

function renderCapabilityDetail(id) {
  const item = capabilities.find((capability) => capability.id === id);
  if (!item) return renderEntityNotFound("能力", id, "#/capabilities");
  const planned = ["planned", "not_implemented"].includes(item.state);
  return el(
    "div",
    {},
    pageHeader("Capability detail", item.name, item.summary, [linkButton("返回能力中心", "#/capabilities", { iconName: "arrow" }), statusChip(capabilityStates[item.state])]),
    planned
      ? callout("info", "当前没有可运行实现", "此页面只说明计划范围，不会采集、推断或展示真实状态。", { iconName: "clock" })
      : callout("warning", "使用范围有限", item.limitation, { iconName: "shield" }),
    el(
      "div",
      { className: "content-grid section" },
      card("职责", "该能力在 Recuvora 中的用途", el("p", { className: "muted" }, item.summary)),
      card("当前事实", "不要从计划推断实现", detailList([
        ["状态", statusChip(capabilityStates[item.state])],
        ["当前可用", item.availableNow],
        ["限制", item.limitation],
      ])),
    ),
    section(
      "进入可用状态前",
      "实现必须同时具备契约、可信装配、失败语义和验证证据。",
      el(
        "div",
        { className: "grid grid-3" },
        card("契约", "输入、输出和范围", el("p", { className: "muted" }, "明确目标、版本、权限、截止时间和 Unknown 语义。")),
        card("运行边界", "故障与资源隔离", el("p", { className: "muted" }, "说明实例、进程、会话、容量和清理责任。")),
        card("验证", "覆盖中断与恢复", el("p", { className: "muted" }, "不能以目录、编译成功或模型声明代替实际验收。")),
      ),
    ),
  );
}

function renderSettings() {
  return el(
    "div",
    {},
    pageHeader("Preferences", "连接与设置", "Web 与 Desktop 共用可信 HTTP API。访问令牌仅保存在当前页面内存，关闭或切换连接后清除。"),
    renderConnectionPanel(),
    el(
      "div",
      { className: "content-grid" },
      el(
        "div",
        { className: "grid" },
        card("界面偏好", "Web 与 Desktop 使用相同设置", el(
          "div",
          { className: "settings-list" },
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "深色运维主题"), el("div", { className: "setting-description" }, "为长时间观察和高信息密度设计。")), tag("固定启用")),
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "状态文字与颜色双重编码"), el("div", { className: "setting-description" }, "状态同时使用文字和颜色。")), tag("固定启用")),
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "降低动态效果"), el("div", { className: "setting-description" }, "自动遵循系统 prefers-reduced-motion。")), tag("跟随系统")),
        )),
        card("安全显示", "外部内容始终作为不可信文本", el(
          "div",
          { className: "settings-list" },
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "模型回复不渲染 Markdown / HTML"), el("div", { className: "setting-description" }, "避免文件、日志或模型文本触发脚本和交互。")), statusChip({ label: "强制", tone: "success" })),
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "服务端决定可用操作"), el("div", { className: "setting-description" }, "按权限、依赖和记录动作清单启用，提交仍由服务核验。")), statusChip({ label: "强制", tone: "success" })),
          el("div", { className: "setting-row" }, el("div", {}, el("div", { className: "setting-title" }, "自动重试副作用"), el("div", { className: "setting-description" }, "Unknown 结果必须先核验。")), statusChip({ label: "关闭", tone: "neutral" })),
        )),
      ),
      el(
        "div",
        { className: "grid" },
        card("运行上下文", isDemo() ? "只读演示" : "服务快照", detailList([
          ["承载方式", shell.label],
          ["共享 UI", shell.sharedUi ? "是" : "否"],
          ["数据模式", statusChip({ label: isDemo() ? "演示数据" : "真实服务", tone: isDemo() ? "warning" : "info" })],
          ["连接状态", connection.status],
          ["活动配置", activeConfiguration ? copyable(activeConfiguration.repairConfig, "复制配置路径") : "未配置"],
          ["状态目录", activeConfiguration ? copyable(activeConfiguration.dataDirectory, "复制状态目录") : "未配置"],
        ])),
        card("配置管理", "尚无图形编辑接口", el("div", {}, el("p", { className: "muted" }, "当前活动配置由可信宿主从项目与目标之外的文件加载。界面不会修改策略、白名单或 Harness registry。"), disabledAction("选择外部配置", { iconName: "folder" }))),
      ),
    ),
  );
}

function renderConnectionPanel() {
  const base = el("input", { id: "service-base", className: "field mono", type: "url", value: defaultServiceBase(), autocomplete: "off", placeholder: "https://host.example" });
  const token = el("input", { id: "service-token", className: "field", type: "password", autocomplete: "off", placeholder: "宿主启动时提供的访问令牌" });
  const connect = () => { intendedRoute = "#/overview"; void authenticateConnection(base, token); };
  return section("服务连接", "仅向明确选择的服务发送令牌；HTTP 限本机回环地址，远程服务必须使用 HTTPS。", card(null, null,
    el("div", { className: "grid" },
      el("div", { className: "form-grid" },
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "service-base" }, "API 服务地址"), base),
        el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "service-token" }, "访问令牌（仅本次页面有效）"), token),
      ),
      el("div", { className: "action-bar-buttons" },
        actionButton("连接真实服务", { variant: "primary", iconName: "activity", onClick: connect }),
        actionButton("刷新快照", { disabled: !api.configured || isDemo(), onClick: startPolling }),
        actionButton("断开并清除令牌", { disabled: !api.configured, onClick: () => selectDemo(false) }),
        actionButton(isDemo() ? "退出演示模式" : "显式进入演示模式", { onClick: () => selectDemo(!isDemo()) }),
      ),
      callout(isDemo() ? "warning" : connection.hasSnapshot ? "info" : "warning", isDemo() ? "演示模式：所有操作禁用" : `连接状态：${connection.status}`, isDemo() ? "演示数据仅在此模式展示；切回真实模式会清空演示记录。" : connection.message),
    ),
  ));
}

function operationAction(label, { permission, available = true, key, path, body, ...options }) {
  const button = actionButton(label, {
    ...options,
    disabled: !can(permission) || !available || pendingRequests.has(key),
    title: !can(permission) || !available ? "当前模式、权限或依赖不允许此操作；请查看连接和能力状态。" : options.title,
    onClick: async () => {
      if (!can(permission) || button.dataset.actionAvailable === "false" || pendingRequests.has(key)) return;
      let payload;
      try { payload = typeof body === "function" ? body() : body; }
      catch (error) { showToast(error.message); return; }
      if (key.startsWith("approval:")) {
        const detail = historyData.detail("approvals", key.slice("approval:".length));
        if (detail.stale || (detail.value && detail.value.revision !== payload.revision)) { showToast("详情版本已变化，请重新读取并审查完整操作。"); return; }
        const action = path.split("/").at(-1);
        if (detail.value && !detail.value.allowedActions?.includes(action)) { showToast("当前记录不允许此操作，请重新读取详情。"); return; }
      }
      pendingRequests.add(key);
      const generation = connectionGeneration;
      const hasOperationId = ["/api/v1/repairs/runs", "/api/v1/simulations"].includes(path) || /^\/api\/v1\/harness\/[^/]+\/runs$/.test(path);
      const temporaryId = hasOperationId ? `ui-${crypto.randomUUID()}` : `submission-${crypto.randomUUID()}`;
      if (hasOperationId) payload = { ...payload, operation_id: temporaryId };
      const context = { path, ...(key.startsWith("approval:") ? { requestId: key.slice("approval:".length), revision: payload.revision } : { operationId: temporaryId }) };
      const uiRoute = parseRoute().path;
      operationViews.set(temporaryId, { id: temporaryId, kind: label, context, uiRoute, status: "submitting", message: "正在发送；尚未收到权威接受回执。" });
      renderApp({ focus: false });
      try {
        const result = await api.submit(path, payload);
        if (generation !== connectionGeneration) return;
        if (result.operation_id) {
          operationViews.delete(temporaryId);
          operationViews.set(result.operation_id, { id: result.operation_id, kind: label, uiRoute, status: "accepted", message: "服务已接受操作，正在读取实际结果。" });
          await refreshOperation(result.operation_id);
        } else {
          // Synchronous decisions are only described using the server receipt.
          operationViews.set(temporaryId, { id: temporaryId, kind: label, context, uiRoute, status: "acknowledged", result, message: "服务已返回回执；正在重新读取权威记录。" });
        }
        startPolling();
      } catch (error) {
        if (generation !== connectionGeneration) return;
        if (error.status === 401) { requireAuthentication(error); return; }
        operationViews.set(temporaryId, { id: temporaryId, kind: label, context, uiRoute, status: error.unknown ? "unknown" : "failed", error: { code: error.code, message: error.message }, result: error.details });
        if (error.status === 409) {
          showToast("记录版本发生冲突，正在重新读取；请审查新 revision 后再次决定。");
          startPolling();
        }
      } finally {
        if (generation === connectionGeneration) {
          pendingRequests.delete(key);
          if (key.startsWith("approval:")) historyData.invalidateDetail("approvals", key.slice("approval:".length));
          while (operationViews.size > 16) operationViews.delete(operationViews.keys().next().value);
          renderApp({ focus: false });
        }
      }
    },
  });
  button.dataset.serviceAction = permission;
  button.dataset.actionAvailable = String(available);
  button.dataset.requestKey = key;
  if (typeof body !== "function") button.dataset.refreshEvents = "true";
  if (key.startsWith("approval:")) {
    button.dataset.approvalId = key.slice("approval:".length);
    button.dataset.revision = String(historyData.detail("approvals", button.dataset.approvalId).value?.revision);
    button.dataset.refreshEvents = "true";
  }
  return button;
}

async function refreshOperation(id) {
  const generation = connectionGeneration;
  try {
    const result = await api.operation(id, { signal: snapshotController?.signal });
    if (generation !== connectionGeneration) return;
    operationViews.set(id, { ...result, id, uiRoute: operationViews.get(id)?.uiRoute });
    while (operationViews.size > 16) operationViews.delete(operationViews.keys().next().value);
  } catch (error) {
    if (generation !== connectionGeneration) return;
    const previous = operationViews.get(id);
    operationViews.set(id, { ...previous, readError: error.message });
  }
}

function renderOperationViews() {
  if (!historyData.page("operations").total && !historyData.page("operations").items.length && !operationViews.size) return null;
  const route = parseRoute();
  const relevant = (operation) => route.section === "logs" || operation.uiRoute === route.path
    || (route.section === "approvals" && route.id && operation.context?.requestId === route.id)
    || (route.section === "repairs" && route.id && operation.context?.taskId === route.id)
    || (route.section === "harnesses" && route.id && operation.context?.harnessId === route.id)
    || (route.section === "simulation" && operation.kind === "simulation");
  const renderCards = (items) => {
    const shown = new Map(items.map((operation) => [operation.id, operation]));
    for (const [id, operation] of operationViews) shown.set(id, operation);
    return el("div", { className: "grid" },
    [...shown.values()].filter(relevant).map((operation) => el("div", { dataset: { patchKey: `receipt:${operation.id}` } }, card(operation.kind ?? "服务操作", operation.id,
      el("div", { className: "grid" },
        statusChip({ label: operation.status ?? "未提供状态", tone: operation.status === "unknown" ? "unknown" : ["failed", "rejected"].includes(operation.status) ? "danger" : ["completed", "succeeded"].includes(operation.status) ? "success" : "info" }),
        el("p", { className: "muted" }, (typeof operation.error === "string" ? operation.error : operation.error?.message) ?? operation.message ?? "状态来自服务操作记录。"),
        operation.readError ? callout("warning", "状态读取失败", `${operation.readError} 上次状态可能已过期；不会重复提交。`) : null,
        operation.status === "unknown" ? callout("unknown", "结果未知", "副作用可能已发生；保留操作 ID 并核验权威记录，不能把未知解释为失败或安全取消。") : null,
        operation.result?.business_verified === false ? businessVerificationChip() : null,
        operation.result ? jsonDetails("展开完整服务结果", operation.result) : null,
        operation.context ? jsonDetails("查看提交关联", operation.context) : null,
        !operation.id.startsWith("submission-") ? el("div", { className: "action-bar-buttons" },
          el("div", { dataset: { patchKey: "read-receipt" } }, actionButton("读取当前操作状态", { disabled: !api.configured, onClick: async () => { await refreshOperation(operation.id); renderApp({ focus: false }); } })),
          operationAction("请求取消", { permission: "operation.cancel", available: ["accepted", "running", "pending"].includes(operation.status), key: `cancel:${operation.id}`, path: `/api/v1/operations/${safeSegment(operation.id)}/cancel`, body: { reason: "操作员通过官方 UI 请求取消" } }),
        ) : null,
      ),
    ))),
    );
  };
  if (route.section === "logs") return el("details", { className: "section", dataset: { patchKey: "all-operation-receipts" } }, el("summary", {}, "宿主操作记录"), renderHistoryList("operations", renderCards));
  const items = historyData.page("operations").items;
  if (![...items, ...operationViews.values()].some(relevant)) return null;
  return section("相关操作回执", "读取服务记录核验当前状态；结果未知时不会重复提交。", renderCards(items), linkButton("查询全部记录", "#/logs"));
}

async function loadLogs(cursor = "") {
  if (logView.loading || (!isDemo() && !can("logs.read"))) return;
  pageController?.abort();
  const controller = new AbortController();
  pageController = controller;
  const generation = connectionGeneration;
  logView.loading = true;
  logView.error = null;
  renderApp({ focus: false });
  try {
    const result = isDemo() ? { items: demoData.logs ?? [], next_cursor: null } : await api.logs({ ...logView.filters, cursor }, { signal: controller.signal });
    if (generation !== connectionGeneration || controller.signal.aborted) return;
    if (!Array.isArray(result.items) || result.items.length > 200) throw new ApiError("invalid_response", "日志响应格式或页大小无效。");
    logView.items = result.items;
    logView.nextCursor = result.next_cursor ?? null;
    logView.cursor = cursor;
    logView.loaded = true;
    logView.readAt = Date.now();
  } catch (error) {
    if (!controller.signal.aborted && generation === connectionGeneration) logView.error = error.message;
  } finally {
    if (generation === connectionGeneration) {
      logView.loading = false;
      if (parseRoute().section === "logs") renderApp({ focus: false });
    }
  }
}

function renderLogs() {
  const filter = (name, label, type = "text") => el("div", { className: "form-group" },
    el("label", { className: "form-label", htmlFor: `log-${name}` }, label),
    el("input", { id: `log-${name}`, className: "field", type, value: logView.filters[name], maxlength: "256", on: { input: (event) => { logView.filters[name] = event.currentTarget.value; } } }),
  );
  const controls = el("div", { className: "form-grid" }, filter("query", "文本查询", "search"), filter("level", "级别（info / warning / error）"), filter("source", "来源"), filter("task_id", "任务 ID"), filter("operation_id", "操作 ID"),
    el("div", { className: "form-group" }, el("label", { className: "form-label", htmlFor: "log-limit" }, "每页条数"), el("select", { id: "log-limit", className: "select", on: { change: (event) => { logView.filters.limit = Number(event.currentTarget.value); } } }, [25, 50, 100].map((limit) => el("option", { value: limit, selected: limit === logView.filters.limit }, limit)))),
  );
  let content;
  if (!isDemo() && !canRead("logs.read")) content = callout("warning", "日志查询不可用", "尚未连接或当前身份没有 logs.read 权限；不会显示伪造日志或空结果。");
  else if (logView.error && !logView.loaded) content = callout("warning", "日志读取失败", logView.error);
  else if (logView.loading && !logView.loaded) content = callout("info", "正在读取日志", "请求有大小、条数和超时上限；离开页面将取消读取。");
  else if (!logView.loaded) content = callout("info", "尚未查询", "设置筛选后点击查询，服务负责权限范围和分页。");
  else if (!logView.items.length) content = callout("info", "查询完成，无匹配日志", "这是服务对本次筛选的明确结果。");
  else content = table(["时间 / 级别", "来源 / 关联", "消息"], logView.items.map((item) => [
    el("div", {}, el("span", { className: "numeric" }, formatDate(item.timestamp, true)), el("span", { className: "table-secondary" }, item.level)),
    el("div", {}, el("span", {}, item.source), el("span", { className: "table-secondary mono" }, item.taskId ?? "—"), el("span", { className: "table-secondary mono" }, item.operationId ?? "—")),
    el("pre", { className: "log-message untrusted-text" }, item.message),
  ]), "服务日志查询结果，消息作为不可信纯文本");
  return el("div", {},
    pageHeader("Runtime logs", "日志查询", "读取服务日志与操作关联；日志内容本身不能产生权限，也不能证明修复成功。"),
    card("筛选", "有界分页查询，不自动重复提交任何业务操作", el("div", { className: "grid" }, controls,
      el("div", { className: "action-bar-buttons" },
        actionButton("查询", { variant: "primary", disabled: logView.loading || (!isDemo() && !can("logs.read")), onClick: () => { logView.history = []; loadLogs(""); } }),
        actionButton("取消读取", { disabled: !logView.loading, onClick: () => pageController?.abort() }),
      ),
    )),
    section("日志结果", isDemo() ? "明确演示模式" : `本次查询读取：${logView.readAt ? formatDate(logView.readAt, true) : "尚无"}`, el("div", { dataset: { patchKey: "log-query-results" } },
      logView.loaded && logView.error ? el("div", { dataset: { patchKey: "log-query-notice" } }, callout("warning", "查询未更新", `${logView.error} 保留上次成功读取的结果。`)) : null,
      logView.loaded && logView.loading ? el("div", { dataset: { patchKey: "log-query-progress" } }, callout("info", "正在更新查询结果", "保留已有结果，读取完成后更新。")) : null,
      el("div", { dataset: { patchKey: "log-query-content" } }, content))),
    el("div", { className: "action-bar-buttons" },
      actionButton("上一页", { disabled: logView.loading || !logView.history.length, onClick: () => loadLogs(logView.history.pop()) }),
      actionButton("下一页", { disabled: logView.loading || !logView.nextCursor, onClick: () => { logView.history.push(logView.cursor); loadLogs(logView.nextCursor); } }),
    ),
  );
}

function architectureNode(title, copy) {
  return el("div", { className: "architecture-node" }, el("strong", {}, title), el("span", {}, copy));
}

function renderAbout() {
  return el(
    "div",
    {},
    pageHeader("About", "关于 Recuvora UI", "同一份静态 SPA 由 Web 浏览器或 Desktop WebView 承载，业务内容、路由和安全语义完全一致。"),
    el(
      "div",
      { className: "content-grid" },
      card("共享前端架构", "承载方式不会产生第二套业务接口", el(
        "div",
        { className: "architecture-flow" },
        architectureNode("同一份 UI 源码", "页面、组件、状态语义与 API client"),
        el("span", { className: "architecture-arrow" }, icon("arrow", 20)),
        architectureNode("Web / Desktop", "浏览器或薄 WebView 壳"),
        el("span", { className: "architecture-arrow" }, icon("arrow", 20)),
        architectureNode("可信 Rust 服务", "唯一的权威视图与提交入口"),
      )),
      card("版本", "当前构建信息", detailList([
        ["Recuvora", appMeta.version],
        ["UI", appMeta.uiVersion],
        ["承载", shell.kind === "desktop" ? "Desktop WebView" : "Web 浏览器"],
        ["数据", isDemo() ? "明确演示模式" : "可信 HTTP 服务快照"],
        ["第三方前端依赖", "无"],
      ])),
    ),
    section(
      "当前限制",
      "产品能力以已实现并验证的 Rust 边界为准。",
      el(
        "div",
        { className: "grid grid-3" },
        card("受限动作", "Windows 本机", el("p", { className: "muted" }, "只允许对白名单内既有 UTF-8 普通文件做不超过 16 KiB 的准确全文替换。")),
        card("没有业务验证", "文件读回不是恢复证据", el("p", { className: "muted" }, "尚未实现目标业务健康检查。")),
        card("本机管理边界", "没有远程身份", el("p", { className: "muted" }, "人工决定当前仅依赖本机操作系统用户与状态目录权限。")),
      ),
    ),
    section(
      "安全原则",
      "UI 不得弱化可信宿主的授权与恢复语义。",
      card(null, null, el(
        "ul",
        { className: "timeline" },
        [
          ["shield", "前端不产生权限", "批准只有在可信服务核验并持久接受后才有效。"],
          ["warning", "Unknown 不能重试", "先核验可能已经发生的动作。"],
          ["file", "外部文本不执行", "模型回复、diff、日志、路径和 JSON 只作纯文本。"],
          ["database", "权威记录不在浏览器", "界面缓存不能恢复任务、策略或审批事实。"],
        ].map(([iconName, title, detail]) => el("li", { className: "timeline-item" }, el("span", { className: "timeline-node" }, icon(iconName, 13)), el("div", { className: "timeline-copy" }, el("div", { className: "timeline-title" }, title), el("div", { className: "timeline-detail" }, detail)))),
      )),
    ),
  );
}

function renderEntityNotFound(kind, id, backHref) {
  return el(
    "div",
    {},
    pageHeader("Not found", `${kind}不存在`, "地址中的标识未匹配当前快照。外部标识只作纯文本展示。"),
    el("div", { className: "card empty-state" }, el("div", {}, el("span", { className: "empty-icon" }, icon("search", 20)), el("h2", {}, "未找到记录"), el("p", {}, `${kind}“${id}”不存在、不可访问或未包含在当前快照中。`), el("div", { className: "section" }, linkButton("返回列表", backHref, { iconName: "arrow" })) )),
  );
}

function renderNotFound() {
  return renderEntityNotFound("页面", parseRoute().path, "#/overview");
}

window.addEventListener("hashchange", () => {
  saveDrafts();
  pageController?.abort();
  projectLogs.deactivate();
  viewState.navOpen = false;
  renderApp();
});

document.addEventListener("click", (event) => {
  const skip = event.target.closest?.(".skip-link");
  if (skip) { event.preventDefault(); document.getElementById("main-content")?.focus(); }
});

document.addEventListener("visibilitychange", () => {
  if (document.hidden) { snapshotController?.abort(); pageController?.abort(); }
  else { startPolling(); const route = parseRoute(); if (route.section === "monitoring" && route.id === "logs") projectLogs.activate(route.action); }
});
window.addEventListener("pagehide", () => { snapshotController?.abort(); pageController?.abort(); projectLogs.clear(); drafts.clear(); api.disconnect(); });

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && viewState.navOpen) {
    viewState.navOpen = false;
    const layout = document.querySelector(".app-layout");
    if (layout) layout.dataset.navOpen = "false";
    const menuButton = document.querySelector(".menu-button");
    if (menuButton) menuButton.focus();
  }
});

if (!location.hash) {
  history.replaceState(null, "", "#/overview");
}
renderApp({ focus: false });
