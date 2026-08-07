/// <reference types="vite/client" />

declare global {
  interface Window {
    __goQueuePluginCss?: (pluginId: string, css: string) => void;
  }
}

export {};
