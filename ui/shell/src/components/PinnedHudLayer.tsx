import type { HudSlot, PluginPanelManifest } from '@glint/plugin-sdk';
import { useOverlay } from '../context/OverlayContext';
import { usePluginApi } from '../context/PluginContext';
import {
  useAppRuntime,
  type LoadedApp,
} from '../context/AppRuntimeContext';
import { AppErrorBoundary } from './AppHost';
import { PluginSurface } from './PluginSurface';

const HUD_SLOTS: HudSlot[] = [
  'top-left',
  'top-right',
  'bottom-left',
  'bottom-right',
  'top-center',
];

interface PinnedAppPanel {
  key: string;
  panel: PluginPanelManifest;
  manifestId: string;
  privileged?: boolean;
}

function AppHudPanel({
  manifestId,
  panelId,
  privileged,
}: {
  manifestId: string;
  panelId: string;
  privileged?: boolean;
}) {
  const { loadedApps } = useAppRuntime();
  const loaded = loadedApps.find((p) => p.manifest.id === manifestId);
  if (!loaded) return null;

  return (
    <AppErrorBoundary>
      <div className="plugin-hud-panel">
        <PluginSurface pluginId={manifestId} privileged={privileged}>
          <LoadedHudPanel loaded={loaded} panelId={panelId} />
        </PluginSurface>
      </div>
    </AppErrorBoundary>
  );
}

/** Hooks must run unconditionally — see LoadedAppPanel in AppShell. */
function LoadedHudPanel({
  loaded,
  panelId,
}: {
  loaded: LoadedApp;
  panelId: string;
}) {
  const api = usePluginApi(loaded.manifest, panelId);
  const { Component } = loaded;
  return <Component api={api} mode="hud" />;
}

export function PinnedHudLayer() {
  const { panelPins } = useOverlay();
  const { loadedApps } = useAppRuntime();

  const pinnedApps: PinnedAppPanel[] = [];
  for (const { manifest } of loadedApps) {
    for (const panel of manifest.panels) {
      const key = `${manifest.id}:${panel.id}`;
      if (panelPins[key]) {
        pinnedApps.push({
          key,
          panel,
          manifestId: manifest.id,
          privileged: manifest.privileged,
        });
      }
    }
  }

  if (pinnedApps.length === 0) {
    return null;
  }

  return (
    <div className="pinned-hud-layer" aria-label="Pinned HUD">
      {HUD_SLOTS.map((slot) => {
        const entries = pinnedApps.filter(
          (entry) => (entry.panel.hudSlot ?? 'top-right') === slot,
        );
        if (entries.length === 0) return null;

        return (
          <div key={slot} className={`hud-slot hud-slot--${slot}`}>
            {entries.map(({ key, panel, manifestId, privileged }) => (
              <AppHudPanel
                key={key}
                manifestId={manifestId}
                panelId={panel.id}
                privileged={privileged}
              />
            ))}
          </div>
        );
      })}
    </div>
  );
}
