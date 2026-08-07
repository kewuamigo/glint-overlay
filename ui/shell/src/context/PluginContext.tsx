import { useEffect, useMemo, useRef } from 'react';
import type {
  MetricsSnapshot,
  PluginAPI,
  PluginManifestSummary,
} from '@glint/plugin-sdk';
import { memoryApiStub, moduleApiStub } from '@glint/plugin-sdk';
import { hostInvoke, pluginInvoke } from '@glint/overlay-bridge';
import { useOverlay } from './OverlayContext';

export interface PluginApiContext {
  targetPid: number | null;
  overlayOpen: boolean;
  panelPins: Record<string, boolean>;
  setPanelPin: (panelKey: string, pinned: boolean) => void;
  getMetrics: () => MetricsSnapshot;
  addMetricsListener: (cb: (m: MetricsSnapshot) => void) => () => void;
}

export function createPluginApi(
  manifest: PluginManifestSummary,
  panelId: string,
  ctx: PluginApiContext,
): PluginAPI {
  const panelKey = `${manifest.id}:${panelId}`;
  const invoke = <T = unknown>(method: string, args: unknown[] = []) =>
    pluginInvoke<T>(manifest.id, method, args);

  return {
    pluginId: manifest.id,
    panelId,
    panels: {
      setPinned: (pinned) => {
        void hostInvoke('panel.setPinned', [panelKey, pinned]);
        ctx.setPanelPin(panelKey, pinned);
      },
      isPinned: () => ctx.panelPins[panelKey] ?? false,
    },
    overlay: {
      open: () => invoke('overlay.open', []),
      close: () => invoke('overlay.close', []),
      isOpen: () => ctx.overlayOpen,
    },
    native: {
      overlay: {
        getGameWindowId: () => invoke('native.overlay.getGameWindowId', []),
        setPosition: (x, y) => invoke('native.overlay.setPosition', [x, y]),
        setAnchor: (x, y) => invoke('native.overlay.setAnchor', [x, y]),
        setMargin: (top, right, bottom, left) =>
          invoke('native.overlay.setMargin', [top, right, bottom, left]),
        listenInput: (cursor, keyboard) =>
          invoke('native.overlay.listenInput', [cursor, keyboard]),
        blockInput: (block) => invoke('native.overlay.blockInput', [block]),
        setBlockingCursor: (cursor) =>
          invoke('native.overlay.setBlockingCursor', [cursor ?? null]),
      },
      metrics: {
        getSnapshot: () => invoke('native.metrics.getSnapshot', []),
      },
      window: {
        getSnapshot: () => invoke('native.window.getSnapshot', []),
        toggleInteractive: () => invoke('native.window.toggleInteractive', []),
        setMode: (mode) => invoke('native.window.setMode', [mode]),
      },
    },
    browser: {
      focus: () => invoke('browser.focus', []),
      blur: () => invoke('browser.blur', []),
      setContentRect: (rect) => invoke('browser.setContentRect', [rect]),
    },
    game: {
      targetPid: ctx.targetPid,
      getProcessInfo: () => invoke('game.getProcessInfo', []),
      onMetrics: (cb) => ctx.addMetricsListener(cb),
      modules: moduleApiStub,
      memory: memoryApiStub,
      saves: {
        getGameDir: () => invoke('game.saves.getGameDir', []),
        getManifestEntry: () => invoke('game.saves.getManifestEntry', []),
        getSaveLocations: () => invoke('game.saves.getSaveLocations', []),
        findGame: (q) => invoke('game.saves.findGame', [q]),
        updateManifest: () => invoke('game.saves.updateManifest', []),
      },
    },
    fs: {
      readText: (path) => invoke('fs.readText', [path]),
      readBytes: async (path) => {
        const data = await invoke<number[]>('fs.readBytes', [path]);
        return data instanceof Uint8Array ? data : Uint8Array.from(data ?? []);
      },
      writeText: (path, content) => invoke('fs.writeText', [path, content]),
      writeBytes: (path, data) => invoke('fs.writeBytes', [path, data]),
      exists: (path) => invoke('fs.exists', [path]),
      isDirectory: (path) => invoke('fs.isDirectory', [path]),
      listDir: (path) => invoke('fs.listDir', [path]),
      pickFolder: () => invoke('fs.pickFolder', []),
      pickFile: () => invoke('fs.pickFile', []),
    },
    storage: {
      get: (key) => invoke('storage.get', [key]),
      set: (key, value) => invoke('storage.set', [key, value]),
      remove: (key) => invoke('storage.remove', [key]),
    },
    db: {
      exec: (sql) => invoke('db.exec', [sql]),
      run: (sql, params) => invoke('db.run', [sql, params ?? []]),
      get: (sql, params) => invoke('db.get', [sql, params ?? []]),
      all: (sql, params) => invoke('db.all', [sql, params ?? []]),
    },
  };
}

export function usePluginApi(
  manifest: PluginManifestSummary,
  panelId: string,
): PluginAPI {
  const {
    targetPid,
    overlayOpen,
    panelPins,
    setPanelPin,
    metrics,
  } = useOverlay();
  const listenersRef = useRef(new Set<(m: MetricsSnapshot) => void>());
  const metricsRef = useRef(metrics);
  metricsRef.current = metrics;
  // Shift+Tab toggles overlayOpen — must NOT recreate `api` or Browser's
  // setContentRect effect cleanup clears the CEF hole and the page vanishes.
  const overlayOpenRef = useRef(overlayOpen);
  overlayOpenRef.current = overlayOpen;
  const panelPinsRef = useRef(panelPins);
  panelPinsRef.current = panelPins;

  useEffect(() => {
    for (const cb of listenersRef.current) {
      cb(metrics);
    }
  }, [metrics]);

  const ctx = useMemo<PluginApiContext>(
    () => ({
      targetPid,
      get overlayOpen() {
        return overlayOpenRef.current;
      },
      get panelPins() {
        return panelPinsRef.current;
      },
      setPanelPin,
      getMetrics: () => metricsRef.current,
      addMetricsListener: (cb) => {
        listenersRef.current.add(cb);
        cb(metricsRef.current);
        return () => {
          listenersRef.current.delete(cb);
        };
      },
    }),
    [targetPid, setPanelPin],
  );

  return useMemo(
    () => createPluginApi(manifest, panelId, ctx),
    [manifest, panelId, ctx],
  );
}
