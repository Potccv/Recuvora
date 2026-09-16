// Shared Web/Desktop HTTP transport. Credentials live only in this instance.
export class ApiError extends Error {
  constructor(code, message, { status = 0, unknown = false, details = null } = {}) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
    this.unknown = unknown;
    this.details = details;
    this.autoRetry = false;
  }
}

export function normalizeBaseUrl(value) {
  let url;
  try { url = new URL(value); } catch { throw new ApiError("invalid_connection", "请输入完整的服务地址。"); }
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && loopback))
    || url.username || url.password || url.search || url.hash || !["", "/"].includes(url.pathname)) {
    throw new ApiError("invalid_connection", "服务地址须为 HTTPS 或本机 HTTP 根地址，不能包含凭据、路径或查询参数。");
  }
  return url.origin;
}

async function readBoundedJson(response, maxBytes) {
  if (!response.headers.get("content-type")?.includes("application/json")) {
    throw new ApiError("invalid_response", "服务没有返回 JSON；请检查 API 地址。");
  }
  if (Number(response.headers.get("content-length")) > maxBytes) {
    await response.body?.cancel();
    throw new ApiError("response_too_large", "响应超过界面读取上限。");
  }
  const reader = response.body?.getReader();
  if (!reader) throw new ApiError("invalid_response", "服务响应缺少内容。");
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let bytes = 0;
  let text = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > maxBytes) throw new ApiError("response_too_large", "响应超过界面读取上限。");
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return JSON.parse(text);
  } catch (error) {
    await reader.cancel().catch(() => {});
    if (error instanceof ApiError) throw error;
    throw new ApiError("invalid_response", "无法读取完整、有效的 JSON 响应。");
  } finally {
    reader.releaseLock();
  }
}

export class ApiClient {
  #baseUrl = "";
  #token = "";
  #fetch;
  #controllers = new Set();

  constructor({ fetchImpl = globalThis.fetch.bind(globalThis), timeoutMs = 12000, maxBytes = 4 * 1024 * 1024 } = {}) {
    this.#fetch = fetchImpl;
    this.timeoutMs = timeoutMs;
    this.maxBytes = maxBytes;
  }

  get configured() { return Boolean(this.#baseUrl && this.#token); }
  get baseUrl() { return this.#baseUrl; }

  connect(baseUrl, token) {
    const normalized = normalizeBaseUrl(baseUrl);
    if (typeof token !== "string" || !token.trim() || token.length > 4096 || /[\r\n]/.test(token)) {
      throw new ApiError("invalid_connection", "请输入有效的访问令牌。");
    }
    this.disconnect();
    this.#baseUrl = normalized;
    this.#token = token.trim();
  }

  disconnect() {
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
    this.#baseUrl = "";
    this.#token = "";
  }

  async request(path, { method = "GET", body, signal } = {}) {
    if (!this.configured) throw new ApiError("unconfigured", "尚未配置可信服务连接。");
    if (!path.startsWith("/api/v1/") || path.includes("#") || !["GET", "POST"].includes(method)) {
      throw new ApiError("invalid_request", "无效的 API 请求。");
    }
    if (signal?.aborted) throw new ApiError("cancelled", "读取已取消。");
    const controller = new AbortController();
    this.#controllers.add(controller);
    const abort = () => controller.abort();
    signal?.addEventListener("abort", abort, { once: true });
    const timeout = setTimeout(abort, this.timeoutMs);
    let dispatched = false;
    try {
      dispatched = true;
      const response = await this.#fetch(`${this.#baseUrl}${path}`, {
        method,
        headers: { Authorization: `Bearer ${this.#token}`, Accept: "application/json", ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
        body: body === undefined ? undefined : JSON.stringify(body),
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
        signal: controller.signal,
      });
      const payload = await readBoundedJson(response, this.maxBytes);
      if (!response.ok) {
        const code = typeof payload?.error?.code === "string" ? payload.error.code : "request_failed";
        const error = new ApiError(code, typeof payload?.error?.message === "string" ? payload.error.message : `服务拒绝请求（${response.status}）。`, {
          status: response.status,
          unknown: code.includes("unknown"),
          details: payload,
        });
        if (response.status === 401) this.onUnauthorized?.(error);
        throw error;
      }
      if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
        throw new ApiError("invalid_response", "服务响应必须为 JSON 对象。");
      }
      return payload;
    } catch (error) {
      if (error instanceof ApiError && error.status) throw error;
      if (method === "POST" && dispatched) {
        throw new ApiError("outcome_unknown", "提交结果未知：请求可能已被接受。请读取权威记录核验，不能自动重试。", { unknown: true, details: { cause: error.code ?? "connection_interrupted" } });
      }
      if (error instanceof ApiError) throw error;
      throw new ApiError(controller.signal.aborted ? "cancelled" : "offline", controller.signal.aborted ? "读取已取消或超时。" : "服务连接中断；保留的快照可能已经过期。");
    } finally {
      clearTimeout(timeout);
      signal?.removeEventListener("abort", abort);
      this.#controllers.delete(controller);
    }
  }

  bootstrap(options) { return this.request("/api/v1/bootstrap", options); }
  list(kind, filters = {}, options) {
    const routes = { repairs: "repairs", approvals: "approvals", "related-approvals": "approvals", operations: "operations", simulation_tasks: "simulations", incidents: "incidents", "overview-incidents": "incidents" };
    if (!routes[kind]) throw new ApiError("invalid_request", "未知历史集合。");
    const query = new URLSearchParams();
    for (const key of ["cursor", "limit", "state", "status", "query", "task_id", "target_id", "monitor_id", "kind"]) {
      if (filters[key] !== undefined && filters[key] !== "") query.set(key, String(filters[key]));
    }
    return this.request(`/api/v1/${routes[kind]}?${query}`, options);
  }
  detail(kind, id, options) {
    if (!["repairs", "approvals", "operations", "monitors", "incidents"].includes(kind)) throw new ApiError("invalid_request", "未知详情集合。");
    return this.request(`/api/v1/${kind}/${encodeURIComponent(id)}`, options);
  }
  operation(id, options) { return this.request(`/api/v1/operations/${encodeURIComponent(id)}`, options); }
  monitors(options) { return this.request("/api/v1/monitors", options); }
  pluginMonitoring(id, options) { return this.request(`/api/v1/monitoring/plugins/${encodeURIComponent(id)}`, options); }
  projectLogs(id, { cursor = "", limit = 32, signal } = {}) {
    const query = new URLSearchParams({ limit: String(limit) });
    if (cursor) query.set("cursor", cursor);
    return this.request(`/api/v1/monitors/${encodeURIComponent(id)}/logs?${query}`, { signal });
  }
  projects(id, { cwd, ...options } = {}) {
    const query = new URLSearchParams({ workspace_id: cwd ?? "" });
    return this.request(`/api/v1/harness/${encodeURIComponent(id)}/projects?${query}`, options);
  }
  submit(path, body, options = {}) { return this.request(path, { ...options, method: "POST", body }); }
  logs(filters, options) {
    const query = new URLSearchParams();
    for (const key of ["level", "source", "task_id", "operation_id", "query", "cursor", "limit"]) {
      if (filters[key] !== undefined && filters[key] !== "") query.set(key, String(filters[key]));
    }
    return this.request(`/api/v1/logs?${query}`, options);
  }
}

export async function pollSnapshots(client, { signal, onSnapshot, onError, intervalMs = 5000, maxPolls = Infinity } = {}) {
  let failures = 0;
  for (let index = 0; index < maxPolls && !signal?.aborted; index++) {
    try { onSnapshot(await client.bootstrap({ signal })); failures = 0; }
    catch (error) {
      if (signal?.aborted) return;
      onError(error);
      failures = Math.min(failures + 1, 4);
      // Authentication and schema failures need an explicit operator correction.
      if ([401, 403].includes(error.status) || ["invalid_response", "response_too_large"].includes(error.code)) return;
    }
    if (index + 1 < maxPolls && !signal?.aborted) await new Promise((resolve) => {
      const done = () => { clearTimeout(timer); signal?.removeEventListener("abort", done); resolve(); };
      const timer = setTimeout(done, Math.min(30000, intervalMs * 2 ** failures));
      signal?.addEventListener("abort", done, { once: true });
      if (signal?.aborted) done();
    });
  }
}
