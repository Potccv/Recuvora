const allowedKinds = new Set(["web", "desktop"]);

function readInjectedShell() {
  const candidate = globalThis.__RECUVORA_SHELL__;
  if (!candidate || typeof candidate !== "object") {
    return null;
  }
  const kind = allowedKinds.has(candidate.kind) ? candidate.kind : null;
  if (!kind) {
    return null;
  }
  return {
    kind,
    label: typeof candidate.label === "string" && candidate.label.length <= 40
      ? candidate.label
      : kind === "desktop" ? "Desktop 壳" : "Web 浏览器",
  };
}

export function getShellContext() {
  const injected = readInjectedShell();
  const isTauri = globalThis.isTauri === true;
  const fallback = isTauri
    ? { kind: "desktop", label: "Desktop 壳" }
    : { kind: "web", label: "Web 浏览器" };
  const shell = injected ?? fallback;
  return Object.freeze({
    ...shell,
    sharedUi: true,
  });
}

export function applyShellMetadata(shell) {
  document.documentElement.dataset.shell = shell.kind;
  document.title = `Recuvora · ${shell.kind === "desktop" ? "Desktop" : "Web"} 控制台`;
}

export async function copyPlainText(value) {
  if (typeof value !== "string") {
    return false;
  }
  if (!navigator.clipboard || typeof navigator.clipboard.writeText !== "function") {
    return false;
  }
  try {
    await navigator.clipboard.writeText(value);
    return true;
  } catch {
    return false;
  }
}
