export { HostIpc, HostEvents } from './channels.js';

export { hostBridge, hostInvoke, pluginInvoke } from './host-client.js';

export type BrowserContentRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type ChromeState = {
  overlayOpen: boolean;
};

/** Host → app push messages (single stream via __goHost / postMessage). */
export type UiMessage =
  | { type: 'metrics'; payload: unknown }
  | { type: 'connection'; connected: boolean; pid?: number }
  | { type: 'chrome'; overlayOpen: boolean }
  /** Opaque native browser process (glint-browser). */
  | {
      type: 'browserSession';
      open: boolean;
      bounds?: BrowserContentRect;
    }
  | { type: 'apps'; manifests: unknown[] }
  | { type: 'extensionTabCreate'; tabId: string; url: string }
  | { type: 'extensionTabSelect'; tabId: string }
  | { type: 'extensionTabRemove'; tabId: string }
  | {
      type: 'browser.navState';
      url: string;
      title: string;
      loading: boolean;
      canGoBack: boolean;
      canGoForward: boolean;
    }
  | {
      type: 'achievement';
      gameName: string;
      title: string;
      iconUrl?: string;
    };

export type HostInvokeRequest = {
  method: string;
  args?: unknown[];
  /** When set, routes to plugin sandbox (permission-checked). */
  pluginId?: string;
};

export type PanelPinPayload = {
  panelKey: string;
  pinned: boolean;
};

/** In-game host bridge: CEF injects `window.__goHost` with invoke + push. */
export type HostBridge = {
  embedded: boolean;
  invoke<T = unknown>(
    method: string,
    args?: unknown[],
    pluginId?: string,
  ): Promise<T>;
};

declare global {
  interface Window {
    __goHost?: HostBridge;
  }
}
