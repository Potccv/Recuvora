// Official monitoring and incident views. Acknowledgement never starts recovery.
import { ApiError } from "./api.js";
import { patchChildren } from "./refresh.js";
import { el, statusChip, safeSegment, formatDate, linkButton, actionButton, pageHeader, section, card, callout, table, detailList, jsonDetails, textFrame, utf8Bytes, copyable } from "./dom.js";
import { knownState, pluginRegistrationStates, renderPluginView, validatePluginPage, validatePluginSummary, validPluginId, viewRegistrationStates, viewRuntimeStates } from "./plugin-monitoring.js";

const healthStates = {
  healthy: { label: "监测条件正常", tone: "success" },
  unhealthy: { label: "监测条件异常", tone: "danger" },
  unknown: { label: "健康状态未知", tone: "unknown" },
};
const freshnessStates = {
  fresh: { label: "样本新鲜", tone: "success" },
  stale: { label: "样本陈旧", tone: "warning" },
  missing: { label: "尚无样本", tone: "neutral" },
};
const coverageStates = {
  complete: { label: "采集完整", tone: "success" },
  partial: { label: "采集不完整", tone: "warning" },
  unavailable: { label: "无法采集", tone: "danger" },
  unknown: { label: "采集范围未知", tone: "unknown" },
};
const incidentStates = {
  open: { label: "待确认收到", tone: "danger" },
  acknowledged: { label: "已确认收到", tone: "warning" },
  resolved: { label: "异常条件已解除", tone: "neutral" },
};
const conditions = {
  active: { label: "异常条件仍存在", tone: "danger" },
  clear: { label: "异常条件已解除", tone: "neutral" },
  unknown: { label: "当前条件未知", tone: "unknown" },
};
const kindLabel = (kind) => kind === "target" ? "目标条件异常" : kind === "coverage" ? "监控采集异常" : "未提供故障类型";
const missing = (value) => value === null || value === undefined || value === "" ? "未提供" : value;
const keyed = (node, key) => { node.dataset.patchKey = key; return node; };
const refreshAction = (label, options) => { const button = actionButton(label, options); button.dataset.refreshEvents = "true"; return button; };
const pluginFingerprint = (plugin) => `${plugin.registration}\0${plugin.view_registration}\0${plugin.registration_error ?? ""}\0${plugin.view_error ?? ""}`;

export class MonitoringConsole {
  constructor({ api, history, can, isDemo, onChange, onDiagnose = () => {}, isConnected = () => true }) {
    Object.assign(this, { api, history, can, isDemo, onChange, onDiagnose, isConnected });
    this.clear();
  }
  clear() {
    this.incidentWidget?.root.replaceChildren();
    for (const page of this.pluginPages?.values() ?? []) page.controller?.abort();
    this.generation = (this.generation ?? 0) + 1;
    this.snapshot = null;
    this.pluginPages = new Map();
    this.pluginCatalogError = null;
    this.monitorError = null;
    this.monitorLoading = false;
    this.receipts = new Map();
    this.drafts = new Map();
    this.incidentWidget = null;
    this.queuedIncidentLoad = null;
    this.lastListRoute = null;
  }
  seed(snapshot, permissions) {
    if (snapshot && !Array.isArray(snapshot.monitors)) throw new ApiError("invalid_response", "监控快照结构不兼容。");
    this.pluginCatalogError = null;
    let normalized = snapshot;
    if (snapshot) {
      const plugins = [];
      if (snapshot.plugins !== undefined && !Array.isArray(snapshot.plugins)) this.pluginCatalogError = new ApiError("invalid_response", "插件监控目录结构不兼容。");
      for (const item of Array.isArray(snapshot.plugins) ? snapshot.plugins : []) {
        try { plugins.push(validatePluginSummary(item)); }
        catch (error) { this.pluginCatalogError = error; }
      }
      normalized = { ...snapshot, plugins };
    }
    this.snapshot = permissions.includes("monitor.read") ? normalized : null;
    this.monitorError = null;
    if (permissions.includes("incident.read") && snapshot?.counts?.incidents) this.history.page("incidents").counts = snapshot.counts.incidents;
    for (const kind of ["monitors", "incidents"]) {
      const permission = kind === "monitors" ? "monitor.read" : "incident.read";
      if (!permissions.includes(permission)) {
        this.history.pages.delete(kind);
        for (const key of this.history.details.keys()) if (key.startsWith(`${kind}:`)) this.history.details.delete(key);
        if (kind === "incidents") { this.drafts.clear(); this.receipts.clear(); this.incidentWidget?.root.replaceChildren(); this.incidentWidget = null; }
      }
    }
    if (!permissions.includes("monitor.read") || !permissions.includes("extension.read")) this.clearPluginPages();
    else {
      const current = new Map((this.snapshot?.plugins ?? []).map((plugin) => [plugin.id, plugin]));
      for (const [id, page] of this.pluginPages) {
        const plugin = current.get(id);
        if (!plugin) { page.controller?.abort(); this.pluginPages.delete(id); continue; }
        const fingerprint = pluginFingerprint(plugin);
        if (page.fingerprint !== fingerprint) {
          page.controller?.abort();
          page.controller = null;
          page.loading = false;
          page.attempted = true;
          page.current = null;
          page.error = null;
          const trustRevoked = plugin.registration === "disabled" || ["invalid", "not_registered"].includes(plugin.view_registration);
          if (trustRevoked) page.value = null;
          page.stale = !trustRevoked && Boolean(page.value);
          page.fingerprint = fingerprint;
        }
      }
    }
  }
  refreshActive(route) {
    if (route.section === "monitoring" && ["monitors", "incidents"].includes(route.id) && route.action) this.ensureDetail(route.id, route.action);
    else if (route.section === "monitoring" && route.id === "plugins" && route.action) this.ensurePlugin(route.action);
    else if (route.section === "monitoring" && !route.id) void this.refreshIncidentsPage();
  }
  async refreshIncidentsPage() {
    const page = this.history.page("incidents");
    if (this.queuedIncidentLoad && !page.loading) { this.drainIncidentLoad(); return; }
    if (!page.loaded || page.loading || !this.can("incident.read") || !this.isConnected()) return;
    const generation = this.generation;
    const previous = [...page.previous];
    await this.history.load("incidents", { cursor: page.cursor, filters: page.filters, direction: "refresh" });
    if (generation !== this.generation || !this.can("incident.read")) return;
    page.previous = previous;
    this.incidentWidget?.draw();
    this.drainIncidentLoad();
  }
  drainIncidentLoad() {
    if (!this.queuedIncidentLoad || !this.incidentWidget || this.history.page("incidents").loading) return;
    const request = this.queuedIncidentLoad;
    this.queuedIncidentLoad = null;
    void this.incidentWidget.load(request.cursor, request.direction, request.filters);
  }
  async refreshMonitors() {
    if (!this.can("monitor.read") || !this.isConnected() || this.monitorLoading) return;
    const generation = this.generation;
    this.monitorLoading = true;
    this.monitorError = null;
    try {
      const result = await this.api.monitors();
      if (generation !== this.generation || !this.can("monitor.read")) return;
      if (!Array.isArray(result.items)) throw new ApiError("invalid_response", "监控列表结构不兼容。");
      const permissions = ["monitor.read", "extension.read", "incident.read"].filter((permission) => this.can(permission));
      this.seed({ configured: result.configured, monitors: result.items, discoveries: result.discoveries ?? [], plugins: result.plugins ?? this.snapshot?.plugins ?? [], runtime_error: result.runtime_error, running: result.running, counts: result.counts }, permissions);
    } catch (error) {
      if (generation === this.generation) this.monitorError = error;
    } finally {
      if (generation === this.generation) { this.monitorLoading = false; this.onChange(); }
    }
  }
  clearPluginPages() {
    for (const page of this.pluginPages?.values() ?? []) page.controller?.abort();
    this.pluginPages?.clear();
  }
  pluginSummary(id) {
    return this.snapshot?.plugins?.find((plugin) => plugin.id === id) ?? null;
  }
  pluginPage(id) {
    if (!this.pluginPages.has(id)) {
      while (this.pluginPages.size >= 8) {
        const oldest = this.pluginPages.keys().next().value;
        this.pluginPages.get(oldest)?.controller?.abort();
        this.pluginPages.delete(oldest);
      }
      const plugin = this.pluginSummary(id);
      this.pluginPages.set(id, {
        attempted: false, loading: false, error: null, current: null, value: null, stale: false, controller: null,
        fingerprint: plugin ? pluginFingerprint(plugin) : "",
      });
    }
    const page = this.pluginPages.get(id);
    this.pluginPages.delete(id);
    this.pluginPages.set(id, page);
    return page;
  }
  ensurePlugin(id) {
    if (!validPluginId(id) || !this.pluginSummary(id)) return null;
    const page = this.pluginPage(id);
    if (!page.attempted && !page.loading) void this.readPlugin(id);
    return page;
  }
  async readPlugin(id, force = false) {
    if (!validPluginId(id) || !this.pluginSummary(id) || !this.can("monitor.read") || !this.can("extension.read") || !this.isConnected()) return;
    const page = this.pluginPage(id);
    if (page.loading || (!force && page.attempted)) return;
    page.controller?.abort();
    const controller = new AbortController();
    const generation = this.generation;
    page.controller = controller;
    page.attempted = true;
    page.loading = true;
    page.error = null;
    try {
      const result = validatePluginPage(await this.api.pluginMonitoring(id, { signal: controller.signal }), id);
      if (generation !== this.generation || controller.signal.aborted || this.pluginPages.get(id) !== page || page.controller !== controller || !this.can("monitor.read") || !this.can("extension.read")) return;
      page.current = result;
      page.fingerprint = pluginFingerprint(result.plugin);
      if (result.view_status === "ready" && result.view) {
        page.value = result;
        page.stale = false;
      } else if (["permission_denied", "not_registered", "invalid_registration"].includes(result.view_status) || result.plugin.registration === "disabled") {
        page.value = null;
        page.stale = false;
      } else {
        page.stale = Boolean(page.value);
      }
    } catch (error) {
      if (generation !== this.generation || controller.signal.aborted || this.pluginPages.get(id) !== page || page.controller !== controller) return;
      page.error = error;
      if (error instanceof ApiError && [401, 403].includes(error.status)) {
        page.current = null;
        page.value = null;
        page.stale = false;
      } else {
        page.stale = Boolean(page.value);
      }
    } finally {
      if (generation === this.generation && this.pluginPages.get(id) === page && page.controller === controller) {
        page.loading = false;
        page.controller = null;
        this.onChange(true);
      }
    }
  }
  render(route) {
    if (this.isDemo()) return el("div", {}, pageHeader("Monitoring & incidents", "监控与故障", "此页面读取真实宿主的监控样本与持久故障。"), callout("info", "演示模式没有真实监控记录", "返回真实连接后读取；此处不会填充模拟故障或提交确认。"));
    if (route.id === "monitors" && route.action) return this.renderMonitor(route.action);
    if (route.id === "incidents" && route.action) return this.renderIncident(route.action);
    if (route.id === "plugins" && route.action) return this.renderPlugin(route.action);
    if (route.id) return callout("warning", "未找到监控页面", "请从监控与故障列表打开具体记录。");
    const listRoute = `${route.path ?? "/monitoring"}?${route.query?.toString() ?? ""}`;
    this.incidentRemount = this.lastListRoute !== listRoute;
    if (this.incidentRemount && route.query?.toString()) {
      const filters = { status: route.query.get("status") || "active" };
      for (const key of ["target_id", "monitor_id", "kind", "query"]) if (route.query.get(key)) filters[key] = route.query.get(key);
      if (this.incidentWidget) { this.incidentWidget.applyFilters(filters); void this.incidentWidget.load("", "refresh", filters); }
      else { this.history.page("incidents").filters = filters; this.history.page("incidents").loaded = false; }
    }
    this.lastListRoute = listRoute;
    return el("div", { className: "grid" },
      pageHeader("Monitoring & incidents", "监控与故障", "监控样本、采集完整性与故障处理状态分别展示。确认收到只记录操作员已知悉。"),
      callout("info", "故障确认与修复验证独立", "确认收到不会解除异常条件或启动修复；异常条件解除也不能作为修复成功或业务恢复验证。"),
      this.renderMonitors(),
      this.renderPluginDirectory(),
      this.can("incident.read") || this.history.page("incidents").loaded ? card("故障记录", "默认列出尚未解除的故障。可按目标、监控、类型与摘要查找。", this.incidentList()) : callout("warning", "无法读取故障", "当前连接或权限不允许读取故障记录。"),
    );
  }
  monitorDimensions(monitor) {
    return el("div", { className: "monitor-dimensions" }, [["目标状态", healthStates[monitor.health]], ["样本时效", freshnessStates[monitor.freshness]], ["采集范围", coverageStates[monitor.coverage]]].map(([label, state]) => el("div", { className: "monitor-dimension" }, el("span", { className: "muted" }, label), statusChip(state))));
  }
  renderDiscoveries(discoveries) {
    if (!discoveries?.length) return null;
    return table(["自动发现 / 扩展", "发现状态", "已登记 / 当前发现", "最近接收", "最近错误"], discoveries.map((discovery) => [
      el("div", {}, discovery.id, el("div", { className: "table-secondary" }, discovery.extension_id)),
      !discovery.running ? "发现已停止" : discovery.complete ? "清单完整" : "清单尚未确认完整",
      `${discovery.known_targets} / ${discovery.present_targets}`,
      formatDate(discovery.last_received_at_ms, true),
      missing(discovery.last_error),
    ]), "监控对象自动发现");
  }
  renderMonitors() {
    if (!this.can("monitor.read") && !this.snapshot) return callout("warning", "无法读取监控", "当前连接或权限不允许读取监控状态；这不表示目标健康。" );
    const snapshot = this.snapshot;
    return card("监控状态", "健康判断、样本新鲜度、采集完整性分别来自宿主。", el("div", { className: "grid" },
      this.monitorError ? callout("warning", "监控读取失败", this.monitorError.message) : null,
      !this.isConnected() ? callout("warning", "监控数据可能陈旧", "连接中断，保留上次读取的监控状态。") : null,
      snapshot?.runtime_error ? callout("danger", "监控运行时不可用", snapshot.runtime_error) : null,
      this.renderDiscoveries(snapshot?.discoveries),
      !snapshot ? callout("warning", "服务尚未提供监控快照", "可读取当前监控配置；未返回状态不能视为健康。")
        : !snapshot.configured ? callout("info", "未配置监控", "宿主尚未配置采样规则；没有真实样本或故障结论。")
          : !snapshot.monitors.length ? callout("warning", "尚未返回监控对象", "已配置不代表已有有效样本。")
            : this.targetGroups(snapshot.monitors),
      actionButton("读取当前监控状态", { disabled: this.monitorLoading || !this.can("monitor.read") || !this.isConnected(), onClick: () => this.refreshMonitors() }),
    ));
  }
  renderPluginDirectory() {
    if (!this.can("monitor.read") && !this.snapshot) return null;
    const plugins = this.snapshot?.plugins ?? [];
    return card("插件监控", "每个插件可登记一个声明式只读专页；宿主标准健康、时效、覆盖与故障仍保留在统一监控中心。", el("div", { className: "grid" },
      this.pluginCatalogError ? callout("warning", "部分插件目录无法展示", this.pluginCatalogError.message) : null,
      plugins.length ? table(["插件", "注册状态", "专属视图", "宿主监控范围", "最近错误"], plugins.map((plugin) => [
        el("a", { href: `#/monitoring/plugins/${safeSegment(plugin.id)}`, className: "table-link" }, plugin.id),
        statusChip(knownState(pluginRegistrationStates, plugin.registration)),
        statusChip(knownState(viewRegistrationStates, plugin.view_registration)),
        `${plugin.monitor_count} 项监控 · ${plugin.target_count} 个目标 · ${plugin.discovery_count} 个发现源`,
        plugin.view_error ?? plugin.registration_error ?? "—",
      ]), "插件监控页面目录") : el("p", { className: "muted" }, "当前宿主没有登记插件监控页面。节点不会作为插件页面列出。"),
    ));
  }
  renderPlugin(id) {
    const plugin = this.pluginSummary(id);
    if (!validPluginId(id) || !plugin) return el("div", { className: "grid" }, pageHeader("Plugin monitoring", "插件监控页不存在", "该插件未登记、不可访问或已从当前快照移除。", [linkButton("返回监控与故障", "#/monitoring")]), callout("warning", "未找到插件监控页", id));
    const page = this.ensurePlugin(id) ?? this.pluginPage(id);
    const current = page.current;
    const successful = page.value;
    const snapshotMonitors = (this.snapshot?.monitors ?? []).filter((monitor) => monitor.owner_plugin_id === id);
    const snapshotDiscoveries = (this.snapshot?.discoveries ?? []).filter((discovery) => discovery.owner_plugin_id === id);
    const declared = successful?.view ?? null;
    const declaredMonitors = successful?.monitors ?? [];
    const runtime = current ? knownState(viewRuntimeStates, current.view_status) : null;
    const statusCopy = current?.view_error ?? page.error?.message ?? plugin.view_error ?? plugin.registration_error;
    return el("div", { className: "grid", "data-patch-key": `plugin-monitoring:${id}` },
      pageHeader("Plugin monitoring", declared?.title ?? id, declared?.summary ?? "插件专属内容与宿主统一监控并列展示。", [linkButton("返回监控与故障", "#/monitoring")]),
      callout("info", "专属内容不是权威健康或授权事实", "插件只能提供有界声明式纯文本展示，不能替换宿主健康、时效、覆盖、故障、授权、修复或业务恢复记录。"),
      card("插件注册", "状态来自宿主登记与当前读取，不从页面内容推断。", detailList([
        ["插件实例", copyable(plugin.id)],
        ["插件注册", statusChip(knownState(pluginRegistrationStates, plugin.registration))],
        ["视图登记", statusChip(knownState(viewRegistrationStates, plugin.view_registration))],
        ["专属读取", runtime ? statusChip(runtime) : page.loading ? "正在读取" : "尚未读取"],
        ["宿主范围", `${plugin.monitor_count} 项监控 · ${plugin.target_count} 个目标 · ${plugin.discovery_count} 个发现源`],
        ["最近读取", current?.read_at ? formatDate(current.read_at, true) : successful?.read_at ? formatDate(successful.read_at, true) : "尚未读取"],
      ]), { footer: statusCopy ?? "没有报告注册或读取错误。" }),
      !this.can("extension.read") ? callout("warning", "无法读取插件专属内容", "当前身份缺少 extension.read；宿主标准监控仍可查看。") : null,
      !this.isConnected() ? callout("warning", "插件专属内容可能陈旧", "连接中断，不会发起读取；保留的宿主快照与上次成功专属内容均不能视为当前状态。") : null,
      page.error ? callout("warning", page.stale ? "专属内容刷新失败，保留上次成功内容" : "专属内容读取失败", page.error.message) : null,
      !page.error && current && current.view_status !== "ready" ? callout(runtime?.tone === "danger" ? "danger" : "warning", runtime?.label ?? "专属视图不可用", current.view_error ?? "宿主没有返回可显示的当前专属内容；标准监控保持可用。") : null,
      page.stale && successful ? callout("warning", "正在显示陈旧的插件专属内容", `上次成功读取：${formatDate(successful.read_at, true)}。这些内容不用于健康或恢复判断。`) : null,
      actionButton("读取当前插件视图", { disabled: page.loading || !this.can("monitor.read") || !this.can("extension.read") || !this.isConnected(), onClick: () => this.readPlugin(id, true) }),
      section("宿主权威监控", "以下健康、样本时效与采集范围来自宿主；插件专属内容不能覆盖这些状态。", snapshotMonitors.length ? this.targetGroups(snapshotMonitors) : callout("warning", "尚无该插件的宿主监控", "没有匹配 owner_plugin_id 的监控记录；这不表示目标健康。")),
      snapshotDiscoveries.length ? section("宿主发现状态", "发现运行和清单完整性由宿主单独展示。", this.renderDiscoveries(snapshotDiscoveries)) : null,
      declared ? renderPluginView(declared, declaredMonitors) : callout("info", "没有可显示的插件专属内容", "插件未登记有效视图、当前不可用或尚未完成首次读取；宿主标准监控仍是可用的降级页面。"),
    );
  }
  targetGroups(monitors) {
    const groups = new Map();
    for (const monitor of monitors) {
      const target = monitor.target_id || "未提供目标";
      if (!groups.has(target)) groups.set(target, []);
      groups.get(target).push(monitor);
    }
    return el("div", { className: "monitor-targets" }, [...groups].map(([target, items]) => {
      const logMonitor = items.find((item) => item.logs_available);
      return keyed(card(target, `${items.length} 项监控 · ${[...new Set(items.map((item) => item.extension_id).filter(Boolean))].join("、") || "未提供来源连接"}`, el("div", { className: "grid" },
        el("div", { className: "action-bar-buttons" },
          refreshAction("查看目标故障", { disabled: !this.can("incident.read"), onClick: () => this.filterTarget(target) }),
          logMonitor ? linkButton("查看实时观测记录", `#/monitoring/logs/${safeSegment(logMonitor.id)}`) : el("span", { className: "muted" }, "观测记录未配置或当前权限无法读取"),
        ),
        table(["监控", "目标、时效与采集", "采样调度", "最近接收", "最近采集错误"], items.map((monitor) => [
          el("div", {}, el("a", { href: `#/monitoring/monitors/${safeSegment(monitor.id)}`, className: "table-link" }, monitor.id), el("div", { className: "table-secondary" }, monitor.source_id)),
          this.monitorDimensions(monitor), monitor.running ? "运行中" : "未运行", formatDate(monitor.last_received_at_ms, true), missing(monitor.last_error),
        ]), `${target} 的监控状态`),
      ), { className: "monitor-target-card" }), `monitor-target:${target}`);
    }));
  }
  incidentTable(items) {
    if (!items.length) return el("p", { className: "muted" }, "当前筛选没有故障记录；这不代表所有目标都已健康。");
    return table(["故障 / 目标", "类型", "处理状态", "观测条件", "出现次数", "最近观测"], items.map((incident) => [
      el("div", {}, el("a", { href: `#/monitoring/incidents/${safeSegment(incident.id)}`, className: "table-link" }, `${incident.target_id}：${incident.summary}`), el("div", { className: "table-secondary" }, incident.monitor_id), el("div", { className: "table-secondary" }, copyable(incident.id, "复制故障 ID"))),
      kindLabel(incident.kind), statusChip(incidentStates[incident.status]), statusChip(conditions[incident.condition]), missing(incident.occurrences), formatDate(incident.last_seen, true),
    ]), "持久故障记录");
  }
  incidentList() {
    if (this.incidentWidget) {
      this.incidentWidget.draw();
      const page = this.history.page("incidents");
      if (!page.loaded && !page.loading && !page.error) void this.incidentWidget.load("", "refresh");
      if (this.incidentWidget.root.isConnected && !this.incidentRemount) return el("div", { "data-live-region": "monitor-incidents", "data-patch-key": "monitor-incidents" });
      return this.incidentWidget.root;
    }
    const page = this.history.page("incidents");
    const host = el("div", { className: "grid" });
    const controls = {
      status: el("select", { className: "select", "aria-label": "故障状态筛选" }, [["active", "尚未解除"], ["open", "待确认收到"], ["acknowledged", "已确认收到"], ["resolved", "异常条件已解除"], ["all", "全部历史"]].map(([value, label]) => el("option", { value }, label))),
      target_id: el("input", { className: "input", placeholder: "目标 ID", "aria-label": "故障目标筛选" }),
      monitor_id: el("input", { className: "input", placeholder: "监控 ID", "aria-label": "故障监控筛选" }),
      kind: el("select", { className: "select", "aria-label": "故障类型筛选" }, [["", "全部类型"], ["target", "目标条件异常"], ["coverage", "监控采集异常"]].map(([value, label]) => el("option", { value }, label))),
      query: el("input", { className: "input", placeholder: "搜索摘要或 ID", "aria-label": "搜索故障摘要或 ID", maxLength: 256 }),
    };
    const applyFilters = (filters) => { for (const [key, control] of Object.entries(controls)) control.value = filters[key] ?? (key === "status" ? "active" : ""); };
    applyFilters(page.filters);
    const readFilters = () => Object.fromEntries(Object.entries(controls).map(([key, control]) => [key, control.value.trim()]).filter(([, value]) => value));
    let lastDraw;
    let filterButton;
    const draw = () => {
      const signature = JSON.stringify([page.items, page.counts, page.total, page.readAt, page.loaded, page.loading, page.error?.message, page.previous, page.next_cursor, this.isConnected(), this.can("incident.read")]);
      if (signature === lastDraw) return;
      lastDraw = signature;
      if (filterButton) filterButton.disabled = page.loading || !this.can("incident.read") || !this.isConnected();
      patchChildren(host, el("div", {}, el("div", { className: "grid" },
      !this.isConnected() ? keyed(callout("warning", "故障数据可能陈旧", "连接中断，保留上次读取的故障列表。"), "incident-list-offline") : null,
      page.error ? keyed(callout("warning", "故障读取失败", `${page.error.message} 保留上次页面，不将错误显示为空结果。`), "incident-list-error") : null,
      page.counts ? el("div", { className: "scope-files", "data-patch-key": "incident-list-counts" }, `全部故障：${page.counts.total ?? "未提供"} · 尚未解除：${page.counts.active ?? "未提供"} · 待确认收到：${page.counts.open ?? "未提供"} · 已确认收到：${page.counts.acknowledged ?? "未提供"} · 条件已解除：${page.counts.resolved ?? "未提供"}`) : null,
      page.loaded ? keyed(this.incidentTable(page.items), "incident-list-results") : el("p", {}, page.loading ? "正在读取故障记录…" : "尚未读取故障记录。"),
      el("div", { className: "action-bar-buttons", "data-patch-key": "incident-list-pagination" },
        el("span", { className: "muted" }, `本页 ${page.items.length} 条 / 匹配 ${page.total} 条 · 列表读取 ${page.readAt ? formatDate(page.readAt) : "尚未读取"}`),
        actionButton("上一页", { disabled: page.loading || !page.previous.length || !this.can("incident.read") || !this.isConnected(), onClick: () => load(page.previous.at(-1), "previous") }),
        actionButton("下一页", { disabled: page.loading || !page.next_cursor || !this.can("incident.read") || !this.isConnected(), onClick: () => load(page.next_cursor, "next") }),
        actionButton("刷新当前筛选首页", { disabled: page.loading || !this.can("incident.read") || !this.isConnected(), onClick: () => load("", "refresh") }),
      ),
    )));
    };
    const load = async (cursor, direction, filters = page.filters) => {
      if (!this.can("incident.read") || !this.isConnected()) return;
      if (page.loading) { this.queuedIncidentLoad = { cursor, direction, filters }; return; }
      const generation = this.generation;
      const pending = this.history.load("incidents", { cursor, direction, filters });
      draw();
      await pending;
      if (generation === this.generation) { draw(); this.drainIncidentLoad(); }
    };
    filterButton = actionButton("筛选故障", { type: "submit", disabled: !this.can("incident.read") });
    const form = el("form", { className: "filter-bar", on: { submit: (event) => { event.preventDefault(); void load("", "refresh", readFilters()); } } }, Object.values(controls), filterButton);
    const root = el("div", { className: "grid", "data-live-region": "monitor-incidents", "data-patch-key": "monitor-incidents" }, form, host);
    this.incidentWidget = { root, draw, load, applyFilters };
    draw();
    if (!page.loaded && !page.loading && !page.error) void load("", "refresh");
    return root;
  }
  filterTarget(target) {
    const filters = { status: "active", target_id: target };
    if (this.incidentWidget) {
      this.incidentWidget.applyFilters(filters);
      void this.incidentWidget.load("", "refresh", filters);
      if (this.incidentWidget.root.isConnected) this.incidentWidget.root.scrollIntoView({ block: "start" });
      else location.hash = "#/monitoring";
    } else {
      this.history.page("incidents").filters = filters;
      this.history.page("incidents").loaded = false;
      location.hash = "#/monitoring";
      this.onChange(true);
    }
  }
  ensureDetail(kind, id) {
    const detail = this.history.detail(kind, id);
    const permission = kind === "incidents" ? "incident.read" : "monitor.read";
    if (this.can(permission) && this.isConnected() && (!detail.value || detail.stale) && !detail.loading && !detail.error) void this.readDetail(kind, id);
    return detail;
  }
  async readDetail(kind, id, force = false) {
    if (!this.can(kind === "incidents" ? "incident.read" : "monitor.read") || !this.isConnected()) return;
    const generation = this.generation;
    this.history.detail(kind, id).stale = true;
    await this.history.readDetail(kind, id);
    if (generation !== this.generation || !this.can(kind === "incidents" ? "incident.read" : "monitor.read")) return;
    const detail = this.history.detail(kind, id);
    const record = detail.value;
    if (record && (record.id !== id || (kind === "incidents" && (!Number.isSafeInteger(record.revision) || !Array.isArray(record.allowed_actions))))) {
      detail.value = null;
      detail.stale = true;
      detail.error = new ApiError("invalid_response", "完整记录的标识、版本或动作清单不兼容。");
      this.onChange(force);
      return;
    }
    if (kind === "incidents" && record?.acknowledgement && this.receipts.get(id)?.status === "unknown") this.receipts.set(id, { status: "acknowledged", message: "已从权威故障记录确认收到记录；异常条件保持独立。" });
    this.onChange(force);
  }
  detailHeader(kind, id, detail) {
    const record = detail.value;
    const title = kind === "incidents" && record ? `${record.target_id}：${record.summary}` : kind === "monitors" && record ? `${record.target_id} · 监控详情` : id;
    return el("div", { className: "grid", "data-patch-key": `${kind}-detail-header:${id}` },
      pageHeader(kind === "incidents" ? "Incident detail" : "Monitor detail", title, "状态由可信宿主返回。", [linkButton("返回监控与故障", "#/monitoring")]),
      !this.isConnected() ? callout("warning", "详情可能陈旧", "连接中断，保留上次读取的记录；当前不能提交确认。") : null,
      detail.error ? callout("warning", "详情读取失败", detail.error.message) : detail.stale ? callout("warning", "详情已陈旧", "正在重新读取当前 revision；更新前不能确认收到。") : null,
      actionButton("读取当前记录（不重复确认）", { disabled: detail.loading || !this.can(kind === "incidents" ? "incident.read" : "monitor.read") || !this.isConnected(), onClick: () => this.readDetail(kind, id, true) }),
    );
  }
  renderMonitor(id) {
    if (!this.can("monitor.read") && !this.history.detail("monitors", id).value) return callout("warning", "无法读取监控详情", "当前连接或权限不允许读取监控状态。");
    const detail = this.ensureDetail("monitors", id);
    const monitor = detail.value;
    return el("div", { className: "grid", "data-patch-key": `monitor-detail:${id}` }, this.detailHeader("monitors", id, detail), !monitor ? el("p", {}, "尚未取得监控详情。") : el("div", { className: "grid", "data-patch-key": `monitor-detail-body:${id}` },
      this.snapshot?.runtime_error ? callout("danger", "监控运行时不可用", this.snapshot.runtime_error) : null,
      this.monitorDimensions(monitor),
      el("div", { className: "action-bar-buttons" },
        monitor.logs_available ? linkButton("查看实时观测记录", `#/monitoring/logs/${safeSegment(monitor.id)}`) : el("span", { className: "muted" }, "观测记录未配置或当前权限无法读取"),
        refreshAction("查看该目标故障", { disabled: !this.can("incident.read"), onClick: () => this.filterTarget(monitor.target_id) }),
      ),
      monitor.last_error ? callout("warning", "最近采样错误", monitor.last_error) : null,
      card("采样上下文", "采样调度运行中不等于采集成功或目标健康。", detailList([
        ["目标", monitor.target_id], ["监控 ID", copyable(monitor.id)], ["来源", monitor.source_id], ["扩展连接", monitor.extension_id], ["采样间隔", `${monitor.interval_ms} ms`],
        ["调度", monitor.running ? "运行中" : "未运行"], ["最近接收", formatDate(monitor.last_received_at_ms, true)],
        ["样本年龄", monitor.sample_age_ms === null || monitor.sample_age_ms === undefined ? "未提供" : `${monitor.sample_age_ms} ms`],
        ["连续失败", missing(monitor.consecutive_failures)], ["连续成功", missing(monitor.consecutive_successes)],
      ])),
      jsonDetails("查看最近样本（不可信纯文本）", monitor.last_value), jsonDetails("查看完整监控快照", monitor),
    ));
  }
  renderIncident(id) {
    if (!this.can("incident.read") && !this.history.detail("incidents", id).value) return callout("warning", "无法读取故障详情", "当前连接或权限不允许读取故障记录。");
    const detail = this.ensureDetail("incidents", id);
    const record = detail.value;
    return el("div", { className: "grid", "data-patch-key": `incident-detail:${id}` }, this.detailHeader("incidents", id, detail), !record ? el("p", {}, "尚未取得完整故障记录。") : el("div", { className: "grid", "data-patch-key": `incident-detail-body:${id}` },
      el("div", { className: "scope-files" }, statusChip(incidentStates[record.status]), statusChip(conditions[record.condition])),
      el("div", { className: "action-bar-buttons" },
        linkButton("查看对应监控", `#/monitoring/monitors/${safeSegment(record.monitor_id)}`),
        linkButton("查看对应观测记录", `#/monitoring/logs/${safeSegment(record.monitor_id)}`),
        refreshAction("准备诊断草稿", { disabled: !this.can("incident.read"), onClick: () => this.onDiagnose(record) }),
      ),
      el("p", { className: "muted" }, "诊断草稿只整理调查信息。修复需匹配已配置的目标与文件范围，并另行申请审批。"),
      callout(record.status === "resolved" ? "info" : "warning", record.status === "resolved" ? "观测到异常条件解除" : "故障状态与确认记录独立", record.status === "resolved" ? "此记录表示监测规则的异常条件已解除；没有据此验证修复成功或业务恢复。" : "确认收到仅记录操作员已知悉；不会解除异常条件、批准操作或启动自动修复。"),
      card("故障上下文", kindLabel(record.kind), detailList([
        ["目标", record.target_id], ["故障 ID", copyable(record.id, "复制故障 ID")], ["监控", el("a", { href: `#/monitoring/monitors/${safeSegment(record.monitor_id)}` }, record.monitor_id)], ["规则", record.rule_id],
        ["Revision", missing(record.revision)], ["首次出现", formatDate(record.first_seen, true)], ["最近观测", formatDate(record.last_seen, true)],
        ["出现次数", missing(record.occurrences)], ["异常条件解除时间", formatDate(record.resolved_at, true)],
      ])),
      textFrame("证据摘要", this.evidenceSummary(record.evidence), "完整证据在下方展开"), jsonDetails("查看完整故障证据", record.evidence),
      this.relatedRepairs(record),
      record.acknowledgement ? card("确认收到记录", "操作员身份由服务端认证并记录。", detailList([["操作员", record.acknowledgement.actor], ["备注", record.acknowledgement.note], ["时间", formatDate(record.acknowledgement.at_ms, true)]])) : null,
      this.acknowledgement(record, detail),
    ));
  }
  relatedRepairs(record) {
    const related = record.related_repairs;
    if (!related || !Array.isArray(related.items)) return null;
    if (related.items.length > 25 || related.items.some((repair) => !repair || typeof repair.id !== "string" || !repair.id)) return keyed(callout("warning", "关联修复结构不兼容", "无法可靠展示此响应的关联修复任务。"), `incident-related-repairs:${record.id}`);
    const items = related.items.slice(0, 25);
    const labels = { completed: "流程已结束", waiting_human: "等待人工决定", blocked: "流程受阻", unknown: "结果未知", canceled: "本轮已取消", failed: "本轮失败", running: "运行中" };
    return keyed(card("关联修复", "修复保存创建时的故障摘要与版本；关联本身不表示异常已解除或业务恢复。", items.length ? table(["修复任务", "目标", "流程状态", "最近更新", "关联故障版本"], items.map((repair) => [
      el("a", { href: `#/repairs/${safeSegment(repair.id)}`, className: "table-link" }, repair.id),
      missing(repair.target), labels[repair.status] ?? missing(repair.status), formatDate(repair.updatedAt, true),
      Number.isSafeInteger(repair.sourceIncident?.revision) ? `revision ${repair.sourceIncident.revision}` : "未提供",
    ]), "与当前故障关联的修复任务") : el("p", { className: "muted" }, "尚无关联修复。"), { footer: items.length ? `显示 ${items.length} 条 / 共 ${Number.isSafeInteger(related.total) ? related.total : items.length} 条` : null }), `incident-related-repairs:${record.id}`);
  }
  evidenceSummary(evidence) {
    if (evidence === undefined || evidence === null) return "服务尚未提供证据。";
    const value = evidence?.value ?? evidence;
    const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
    return text.length > 1600 ? `${text.slice(0, 1600)}\n[摘要已截断，请展开完整证据]` : text;
  }
  acknowledgement(record, detail) {
    const receipt = this.receipts.get(record.id);
    const allowed = this.can("incident.acknowledge") && this.isConnected() && !detail.stale && !detail.loading && record.status === "open" && record.allowed_actions?.includes("acknowledge") && Number.isSafeInteger(record.revision) && !["submitting", "unknown"].includes(receipt?.status);
    const noteId = `incident-note-${safeSegment(record.id)}`;
    const note = el("textarea", { id: noteId, className: "textarea", "aria-label": "确认收到备注", value: this.drafts.get(record.id) ?? "", placeholder: "可选：说明已知悉的异常与当前处理安排" });
    const counter = el("span", { className: "byte-counter" });
    const validateNote = () => {
      const bytes = utf8Bytes(note.value);
      counter.textContent = `${bytes} / 4096 字节（可选）`;
      note.setCustomValidity(bytes <= 4096 && !note.value.includes("\0") ? "" : "备注最多 4096 个 UTF-8 字节，不能包含 NUL。");
    };
    validateNote();
    note.addEventListener("input", () => {
      validateNote();
      this.drafts.set(record.id, note.value);
      while (this.drafts.size > 8) this.drafts.delete(this.drafts.keys().next().value);
    });
    const submit = refreshAction("确认收到", { disabled: !allowed, variant: "primary", onClick: () => this.acknowledge(record, document.getElementById(noteId) ?? note) });
    submit.dataset.serviceAction = "incident.acknowledge";
    submit.dataset.incidentId = record.id;
    submit.dataset.revision = String(record.revision);
    return keyed(card("确认收到", "只确认已知悉，不解除故障，不执行修复。", el("div", { className: "grid" },
      receipt ? callout(receipt.status === "unknown" ? "unknown" : receipt.status === "failed" ? "warning" : "info", receipt.status === "unknown" ? "确认回执未知" : receipt.status === "submitting" ? "正在提交确认" : "确认收到回执", receipt.message) : null,
      record.status === "open" ? el("div", { className: "grid", "data-patch-key": `incident-note-form:${record.id}` }, note, counter, el("p", { className: "muted" }, `确认绑定 ${record.id} · revision ${record.revision}；宿主重新核验权限和版本。`), submit) : el("p", { className: "muted" }, "当前记录不需要再次确认收到。"),
    )), `incident-ack:${record.id}`);
  }
  synchronizeActions() {
    for (const button of document.querySelectorAll("[data-incident-id]")) {
      const detail = this.history.detail("incidents", button.dataset.incidentId);
      const record = detail.value;
      const receipt = this.receipts.get(button.dataset.incidentId);
      button.disabled = !this.can("incident.acknowledge") || !this.isConnected() || detail.stale || detail.loading || !record || String(record.revision) !== button.dataset.revision || record.status !== "open" || !record.allowed_actions?.includes("acknowledge") || ["submitting", "unknown"].includes(receipt?.status);
    }
  }
  async acknowledge(rendered, note) {
    const detail = this.history.detail("incidents", rendered.id);
    const record = detail.value;
    const receipt = this.receipts.get(rendered.id);
    if (!this.can("incident.acknowledge") || !this.isConnected() || !note || detail.stale || detail.loading || !record || record.revision !== rendered.revision || record.status !== "open" || !record.allowed_actions?.includes("acknowledge") || ["submitting", "unknown"].includes(receipt?.status)) return;
    const draft = this.drafts.get(record.id) ?? note.value;
    if (!note.reportValidity() || utf8Bytes(draft) > 4096 || draft.includes("\0")) return;
    const generation = this.generation;
    this.drafts.set(record.id, draft);
    this.receipts.set(record.id, { status: "submitting", message: "正在提交；尚未收到权威回执。" });
    while (this.receipts.size > 16) this.receipts.delete(this.receipts.keys().next().value);
    this.onChange(true);
    try {
      const result = await this.api.submit(`/api/v1/incidents/${safeSegment(record.id)}/acknowledge`, { revision: record.revision, note: draft.trim() });
      if (generation !== this.generation || !this.can("incident.read")) return;
      if (result.id !== record.id || !Number.isSafeInteger(result.revision) || result.revision < record.revision || !result.acknowledgement || !["acknowledged", "resolved"].includes(result.status)) throw new ApiError("outcome_unknown", "无法确认完整故障回执。", { unknown: true });
      this.receipts.set(record.id, { status: "acknowledged", message: "宿主已返回确认收到记录；异常条件没有因该确认而解除。" });
      if (!Number.isSafeInteger(detail.value?.revision) || detail.value.revision <= result.revision) detail.value = result;
      detail.stale = false;
      this.history.page("incidents").loaded = false;
    } catch (error) {
      if (generation !== this.generation || !this.can("incident.read")) return;
      this.receipts.set(record.id, { status: error.unknown ? "unknown" : "failed", message: error.unknown ? "确认可能已被接受。请读取当前故障记录核验，不会重复提交。" : error.message });
      if (error.unknown || error.status === 409) detail.stale = true;
      if (error.status === 409) await this.readDetail("incidents", record.id);
    } finally {
      if (generation === this.generation) this.onChange(true);
    }
  }
}
