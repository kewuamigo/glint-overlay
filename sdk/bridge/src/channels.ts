/** Launcher Electron IPC channel names (launcher only — not used by in-game apps). */

/** Renderer → host: one invoke dispatcher (host UI + plugin sandbox). */
export const HostIpc = {
  invoke: 'host:invoke',
} as const;

/** Host → renderer: one push stream; in-game CEF mirrors to window.postMessage. */
export const HostEvents = {
  push: 'host:push',
} as const;
