import type { HostBridge } from './index.js';

/** In-game CEF overlay bridge; undefined outside the overlay host. */
export function hostBridge(): HostBridge | undefined {
  return window.__goHost;
}

/** Invoke a host method via `__goHost` (CEF-backed in-game; no Electron required). */
export function hostInvoke<T = unknown>(
  method: string,
  args: unknown[] = [],
): Promise<T | undefined> {
  return hostBridge()?.invoke<T>(method, args) ?? Promise.resolve(undefined);
}

/** Invoke a plugin-scoped method via `__goHost` with permission checks. */
export function pluginInvoke<T = unknown>(
  pluginId: string,
  method: string,
  args: unknown[] = [],
): Promise<T | undefined> {
  return hostBridge()?.invoke<T>(method, args, pluginId) ?? Promise.resolve(undefined);
}
