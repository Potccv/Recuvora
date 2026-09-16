// Reconcile the current view without replacing the shell, focused controls or
// unchanged rows. This module has no business rules and never submits requests.
function key(node) {
  if (node.nodeType !== 1) return null;
  return node.getAttribute("data-patch-key") || node.id || null;
}

function sameKind(a, b) {
  return a.nodeType === b.nodeType && a.nodeName === b.nodeName;
}

function syncAttributes(current, next) {
  for (const attribute of [...current.attributes]) {
    if (!next.hasAttribute(attribute.name)) current.removeAttribute(attribute.name);
  }
  for (const attribute of [...next.attributes]) {
    if (current.getAttribute(attribute.name) !== attribute.value) current.setAttribute(attribute.name, attribute.value);
  }
  if ("disabled" in next && current.disabled !== next.disabled) current.disabled = next.disabled;
  if ("checked" in next && current.checked !== next.checked && current.type !== "checkbox") current.checked = next.checked;
  // Dynamic action callbacks read mounted controls by ID. Static form callbacks
  // retain their original, mounted control references.
  if (next.dataset.refreshEvents === "true") {
    for (const [name, listener] of Object.entries(current.__recuvoraListeners ?? {})) current.removeEventListener(name, listener);
    current.__recuvoraListeners = { ...(next.__recuvoraListeners ?? {}) };
    for (const [name, listener] of Object.entries(current.__recuvoraListeners)) current.addEventListener(name, listener);
  }
}

export function patchNode(current, next) {
  if (current === next) return current;
  if (!sameKind(current, next) || (key(current) && key(next) && key(current) !== key(next))) {
    current.replaceWith(next);
    return next;
  }
  if (current.nodeType === 3) {
    if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
    return current;
  }
  if (current.nodeType !== 1) return current;
  if (current.dataset.liveRegion && current.dataset.liveRegion === next.dataset.liveRegion) return current;
  if (current.dataset.preserveView === "file-diff" && next.dataset.preserveView === "file-diff") {
    if (current.dataset.requestId === next.dataset.requestId && current.dataset.revision === next.dataset.revision) return current;
    current.replaceWith(next);
    return next;
  }
  const editable = ["INPUT", "TEXTAREA", "SELECT"].includes(current.tagName);
  const value = editable ? current.value : null;
  const open = current.tagName === "DETAILS" ? current.open : null;
  syncAttributes(current, next);
  if (current.tagName !== "TEXTAREA") patchChildren(current, next);
  if (editable) {
    if (current.tagName !== "SELECT" || [...current.options].some((option) => option.value === value)) current.value = value;
  }
  if (open !== null) current.open = open;
  return current;
}

export function patchChildren(current, next) {
  const oldNodes = [...current.childNodes];
  const keyed = new Map(oldNodes.map((node) => [key(node), node]).filter(([id]) => id));
  const retained = new Set();
  let position = current.firstChild;
  for (const candidate of [...next.childNodes]) {
    const id = key(candidate);
    let existing = id ? keyed.get(id) : position;
    if (existing && (retained.has(existing) || (!id && key(existing)) || !sameKind(existing, candidate))) existing = null;
    const wasAtPosition = Boolean(existing && existing === position);
    const mounted = existing ? patchNode(existing, candidate) : candidate;
    // A versioned view can replace itself while being patched. Its old node is
    // no longer a valid insertBefore reference in the real DOM.
    if (wasAtPosition) position = mounted;
    if (mounted !== position) current.insertBefore(mounted, position);
    retained.add(mounted);
    position = mounted.nextSibling;
  }
  for (const node of [...current.childNodes]) if (!retained.has(node)) node.remove();
}
