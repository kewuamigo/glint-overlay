import { Component, useEffect, useRef, type ReactNode } from 'react';
import type {
  PluginComponent,
  PluginManifestSummary,
} from '@glint/plugin-sdk';
import { useOverlay } from '../context/OverlayContext';
import {
  type LoadedApp,
  useAppRuntime,
} from '../context/AppRuntimeContext';

class AppErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="plugin-slot" style={{ color: 'var(--danger)' }}>
          App error: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

async function loadApp(
  manifest: PluginManifestSummary,
): Promise<{ ok: LoadedApp } | { ok: false; error: string }> {
  try {
    window.__goActivePluginId = manifest.id;
    const mod = await import(/* @vite-ignore */ manifest.bundleUrl);
    const Component = (mod.Plugin ?? mod.default) as PluginComponent | undefined;
    if (!Component) {
      return { ok: false, error: `App ${manifest.id}: no Plugin or default export` };
    }
    return { ok: true, manifest, Component };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.warn(`Failed to load app ${manifest.id}:`, err);
    return { ok: false, error: message };
  } finally {
    if (window.__goActivePluginId === manifest.id) {
      delete window.__goActivePluginId;
    }
  }
}

export function AppHost() {
  const { manifests, setLoadedApps, setLoadError } = useAppRuntime();
  const { setPanelPin, panelPins } = useOverlay();

  // Read panelPins without re-running the effect when it changes.
  // The guard below only needs the value as of when manifests first arrive —
  // that's the moment we decide whether to honour defaultPinned.
  const panelPinsRef = useRef(panelPins);
  panelPinsRef.current = panelPins;

  useEffect(() => {
    if (manifests.length === 0) {
      setLoadedApps([]);
      return;
    }

    let cancelled = false;

    for (const manifest of manifests) {
      for (const panel of manifest.panels) {
        if (panel.pinnable && panel.defaultPinned) {
          const key = `${manifest.id}:${panel.id}`;
          // Only apply defaultPinned if the user has not explicitly set this
          // panel's pin state (key absent = never touched). Without the guard
          // the effect would re-apply defaultPinned:true on every refresh,
          // making it impossible to unpin a panel during a session.
          if (!(key in panelPinsRef.current)) {
            setPanelPin(key, true);
          }
        }
      }
    }

    void Promise.all(
      manifests.map(async (manifest) => {
        // Opaque native apps (Browser) — dock icon only; no Electron Plugin mount.
        if (manifest.native) {
          setLoadError(manifest.id, null);
          return null;
        }
        const result = await loadApp(manifest);
        if (cancelled) return null;
        if (!result.ok) {
          setLoadError(manifest.id, result.error);
          return null;
        }
        setLoadError(manifest.id, null);
        return result;
      }),
    ).then((results) => {
      if (cancelled) return;
      setLoadedApps(
        results
          .filter((p): p is Extract<typeof p, { ok: true }> => p !== null && p.ok)
          .map(({ manifest, Component }) => ({ manifest, Component })),
      );
    });

    return () => {
      cancelled = true;
    };
  }, [manifests, setLoadedApps, setPanelPin, setLoadError]);

  return null;
}

export { AppErrorBoundary };
