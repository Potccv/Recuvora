// Bounded, memory-only read models. This module never submits business actions.
import { ApiError } from "./api.js";

export class HistoryStore {
  constructor(client) { this.client = client; this.clear(); }
  clear() {
    this.generation = (this.generation ?? 0) + 1;
    this.pages = new Map();
    this.details = new Map();
  }
  page(name) {
    if (!this.pages.has(name)) this.pages.set(name, { items: [], total: 0, next_cursor: null, cursor: "", previous: [], filters: name === "approvals" ? { state: "attention" } : name === "incidents" ? { status: "active" } : {}, loaded: false, loading: false, error: null, readAt: null, counts: null });
    return this.pages.get(name);
  }
  seed(snapshot) {
    for (const [key, detail] of this.details) {
      if (["approvals:", "repairs:", "incidents:", "monitors:"].some((kind) => key.startsWith(kind))) detail.stale = true;
    }
    for (const name of ["repairs", "approvals", "operations", "simulation_tasks"]) {
      const page = this.page(name);
      // Polling updates only the unfiltered first page. Paging/search remain stable.
      const defaultFilters = name === "approvals" ? { state: "attention" } : {};
      if (!page.loading && !page.cursor && JSON.stringify(page.filters) === JSON.stringify(defaultFilters)) {
        Object.assign(page, snapshot.pages?.[name] ?? {}, { items: snapshot[name] ?? [], loaded: true, error: null, readAt: snapshot.runtime?.updatedAt ?? Date.now() });
      }
    }
  }
  async load(name, { cursor = "", filters, direction = "refresh", signal } = {}) {
    const page = this.page(name);
    if (page.loading) return;
    const generation = this.generation;
    page.loading = true;
    page.error = null;
    const nextFilters = filters ?? page.filters;
    try {
      const result = await this.client.list(name, { ...nextFilters, cursor, limit: 25 }, { signal });
      if (generation !== this.generation) return;
      if (!Array.isArray(result.items) || result.items.length > 100 || !Number.isSafeInteger(result.total)) throw new ApiError("invalid_response", "历史页结构不兼容。");
      const previous = direction === "next" ? [...page.previous, page.cursor] : direction === "previous" ? page.previous.slice(0, -1) : [];
      Object.assign(page, result, { cursor, filters: nextFilters, previous, loaded: true, readAt: result.read_at ?? Date.now() });
    } catch (error) {
      if (generation === this.generation) page.error = error;
    } finally {
      if (generation === this.generation) page.loading = false;
    }
  }
  detail(kind, id) {
    const key = `${kind}:${id}`;
    if (!this.details.has(key)) {
      while (this.details.size >= 8) this.details.delete(this.details.keys().next().value);
      this.details.set(key, { value: null, loading: false, error: null, stale: false, readAt: null });
    }
    return this.details.get(key);
  }
  invalidateDetail(kind, id) { this.details.delete(`${kind}:${id}`); }
  readDetail(kind, id, { signal } = {}) {
    const detail = this.detail(kind, id);
    if (detail.pending) return detail.pending;
    const generation = this.generation;
    detail.loading = true;
    detail.error = null;
    detail.pending = (async () => {
      try {
        const value = await this.client.detail(kind, id, { signal });
        if (generation === this.generation) {
          // An observation read started before an acknowledgement can finish
          // after its receipt. It must not replace a newer incident revision.
          const olderIncident = kind === "incidents" && Number.isSafeInteger(detail.value?.revision) && Number.isSafeInteger(value?.revision) && value.revision < detail.value.revision;
          if (!olderIncident) detail.value = value;
          detail.readAt = Date.now();
          detail.stale = false;
        }
      } catch (error) {
        if (generation === this.generation) detail.error = error;
      } finally {
        if (generation === this.generation) { detail.loading = false; detail.pending = null; }
      }
    })();
    return detail.pending;
  }
}
