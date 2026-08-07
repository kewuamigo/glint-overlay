const pluginCssById = new Map<string, string>();
const appliedToTarget = new WeakMap<ShadowRoot | HTMLElement, Set<string>>();

export function queuePluginCss(pluginId: string, css: string): void {
  const existing = pluginCssById.get(pluginId);
  pluginCssById.set(pluginId, existing ? `${existing}\n${css}` : css);
}

export function applyQueuedPluginStyles(
  pluginId: string,
  target: ShadowRoot | HTMLElement,
): void {
  const css = pluginCssById.get(pluginId);
  if (!css) return;

  let applied = appliedToTarget.get(target);
  if (!applied) {
    applied = new Set();
    appliedToTarget.set(target, applied);
  }
  if (applied.has(pluginId)) return;

  const style = document.createElement('style');
  style.setAttribute('data-app-id', pluginId);
  style.textContent = css;
  target.insertBefore(style, target.firstChild);
  applied.add(pluginId);
}

export function installPluginStyleBridge(): void {
  window.__goQueuePluginCss = queuePluginCss;
}

declare global {
  interface Window {
    __goActivePluginId?: string;
    __goQueuePluginCss?: (pluginId: string, css: string) => void;
  }
}
