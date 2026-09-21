import { useMemo } from 'react';
import { hostInvoke } from '@glint/overlay-bridge';
import { useOverlay } from '../context/OverlayContext';
import { usePluginApi } from '../context/PluginContext';
import {
  useAppRuntime,
  type LoadedApp,
} from '../context/AppRuntimeContext';
import { AppWindow } from './AppWindow';
import { OverlayDock } from './OverlayDock';
import { AppErrorBoundary } from './AppHost';
import { PluginSurface } from './PluginSurface';
import {
  useOverlayWindows,
  type AppPanelTab,
} from '../hooks/useOverlayWindows';
import { useSessionInfo } from '../hooks/useSessionInfo';

function AppFullPanel({
  manifestId,
  panelId,
  privileged,
}: {
  manifestId: string;
  panelId: string;
  privileged?: boolean;
}) {
  const { loadedApps, loadErrors } = useAppRuntime();
  const loaded = loadedApps.find((p) => p.manifest.id === manifestId);
  const loadError = loadErrors[manifestId];
  if (!loaded) {
    return (
      <div className="plugin-slot muted">
        {loadError ?? 'Loading app…'}
      </div>
    );
  }

  return (
    <AppErrorBoundary>
      <PluginSurface pluginId={manifestId} privileged={privileged}>
        <LoadedAppPanel loaded={loaded} panelId={panelId} />
      </PluginSurface>
    </AppErrorBoundary>
  );
}

function LoadedAppPanel({
  loaded,
  panelId,
}: {
  loaded: LoadedApp;
  panelId: string;
}) {
  const api = usePluginApi(loaded.manifest, panelId);
  const { Component } = loaded;
  return <Component api={api} mode="full" />;
}

export function AppShell({ suspended = false }: { suspended?: boolean }) {
  const { connected, targetPid } = useOverlay();
  const { manifests } = useAppRuntime();
  const { clock, session, total, gameName } = useSessionInfo();

  const appTabs = useMemo<AppPanelTab[]>(() => {
    const tabs: AppPanelTab[] = [];
    for (const manifest of manifests) {
      for (const panel of manifest.panels) {
        tabs.push({
          key: `${manifest.id}:${panel.id}`,
          title: panel.title,
          manifestId: manifest.id,
          panelId: panel.id,
          privileged: manifest.privileged,
          iconUrl: manifest.iconUrl,
        });
      }
    }
    return tabs;
  }, [manifests]);

  const {
    windows,
    focusedKey,
    setFocusedKey,
    toggleWindow,
    closeWindow,
    minimizeWindow,
    bringToFront,
    updateBounds,
    minWidth,
    minHeight,
    maxWidth,
    maxHeight,
  } = useOverlayWindows();

  const openKeys = useMemo(
    () => new Set(windows.map((w) => w.key)),
    [windows],
  );

  const handleToggle = (tab: AppPanelTab) => {
    if (tab.manifestId === 'browser') {
      const existing = windows.find((w) => w.key === tab.key);
      if (existing && !existing.minimized && focusedKey === tab.key) {
        // Close UI first — never block on closeSession (stuck CEF left the
        // window unclosable when openSession never paired).
        closeWindow(tab.key);
        setFocusedKey(null);
        void hostInvoke('browser.closeSession');
        return;
      }
      void hostInvoke('browser.openSession').then(() => {
        toggleWindow(tab);
      });
      return;
    }
    toggleWindow(tab);
  };

  const handleClose = (win: (typeof windows)[number]) => {
    if (win.manifestId === 'browser') {
      closeWindow(win.key);
      void hostInvoke('browser.closeSession');
      return;
    }
    closeWindow(win.key);
  };

  return (
    <div
      className={
        [
          'overlay-root',
          'overlay-desktop',
          suspended ? 'overlay-desktop-suspended' : '',
        ]
          .filter(Boolean)
          .join(' ')
      }
      aria-hidden={suspended}
    >
      <div className="overlay-desktop-chrome">
        <span className="overlay-desktop-eyebrow">Live session</span>
        <span className="overlay-desktop-title">Glint</span>
        <span className="overlay-desktop-status">
          {connected ? `PID ${targetPid}` : 'Disconnected'}
        </span>
        <span className="chrome-info">
          <span className="chrome-chip tabular-nums">{clock}</span>
          {session && (
            <span className="chrome-chip tabular-nums">{session}</span>
          )}
          {total && (
            <span
              className="chrome-chip tabular-nums"
              title={gameName ?? undefined}
            >
              {total}
            </span>
          )}
        </span>
        <span className="hotkey-hint">Shift+Tab</span>
      </div>

      <div className="overlay-window-layer">
        {windows.map((win) => (
          <AppWindow
            key={win.key}
            title={win.title}
            bounds={win.bounds}
            zIndex={win.zIndex}
            minimized={win.minimized}
            focused={focusedKey === win.key}
            privileged={win.privileged}
            minWidth={minWidth}
            minHeight={minHeight}
            maxWidth={maxWidth}
            maxHeight={maxHeight}
            onFocus={() => bringToFront(win.key)}
            onClose={() => handleClose(win)}
            onMinimize={() => minimizeWindow(win.key)}
            onBoundsChange={(bounds) => updateBounds(win.key, bounds)}
          >
            <div
              className={
                win.manifestId === 'browser'
                  ? 'overlay-window-panel app-panel-host browser-panel-host'
                  : 'overlay-window-panel app-panel-host'
              }
            >
              <AppFullPanel
                key={win.key}
                manifestId={win.manifestId}
                panelId={win.panelId}
                privileged={win.privileged}
              />
            </div>
          </AppWindow>
        ))}
      </div>

      <OverlayDock
        tabs={appTabs}
        openKeys={openKeys}
        focusedKey={focusedKey}
        onToggle={handleToggle}
      />
    </div>
  );
}
