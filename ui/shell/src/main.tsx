import * as React from 'react';
import * as ReactJSXRuntime from 'react/jsx-runtime';
import { StrictMode, useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import type { PluginManifestSummary } from '@glint/plugin-sdk';
import { hostInvoke } from '@glint/overlay-bridge';
import { OverlayProvider, useOverlay } from './context/OverlayContext';
import { AppRuntimeProvider, useAppRuntime } from './context/AppRuntimeContext';
import { AppShell } from './components/AppShell';
import { AppHost } from './components/AppHost';
import { PinnedHudLayer } from './components/PinnedHudLayer';
import { AchievementToasts } from './components/AchievementToasts';
import './tailwind.css';
import './styles.css';
import { installPluginStyleBridge } from './plugin-styles';

declare global {
  interface Window {
    __goSharedReact?: typeof React;
    __goSharedJsxRuntime?: typeof ReactJSXRuntime;
  }
}

function sortManifests(manifests: PluginManifestSummary[]): PluginManifestSummary[] {
  return [...manifests].sort((a, b) => {
    const aBuiltin = a.builtin ? 0 : 1;
    const bBuiltin = b.builtin ? 0 : 1;
    if (aBuiltin !== bBuiltin) return aBuiltin - bBuiltin;
    return a.name.localeCompare(b.name);
  });
}

window.__goSharedReact = React;
window.__goSharedJsxRuntime = ReactJSXRuntime;
installPluginStyleBridge();

function HostBridge() {
  const { setMetrics, setConnected, setOverlayOpen } = useOverlay();
  const { setManifests } = useAppRuntime();

  useEffect(() => {
    void hostInvoke<PluginManifestSummary[]>('apps.list').then((manifests) => {
      if (Array.isArray(manifests)) {
        setManifests(sortManifests(manifests));
      }
    });

    const handler = (event: MessageEvent) => {
      if (event.data?.type === 'metrics') {
        setMetrics(event.data.payload);
      }
      if (event.data?.type === 'connection') {
        setConnected(event.data.connected, event.data.pid, event.data);
      }
      if (event.data?.type === 'chrome') {
        setOverlayOpen(Boolean(event.data.overlayOpen));
      }
      if (event.data?.type === 'apps') {
        const manifests = event.data.manifests as PluginManifestSummary[] | undefined;
        if (Array.isArray(manifests)) {
          setManifests(sortManifests(manifests));
        }
      }
    };
    window.addEventListener('message', handler);
    return () => window.removeEventListener('message', handler);
  }, [setMetrics, setConnected, setOverlayOpen, setManifests]);

  return null;
}

function App() {
  const { overlayOpen, anyPinned } = useOverlay();

  return (
    <>
      <HostBridge />
      <AppHost />
      <AchievementToasts />
      {!overlayOpen && anyPinned && <PinnedHudLayer />}
      {/* Keep AppShell mounted so window layout survives Shift+Tab. */}
      <AppShell suspended={!overlayOpen} />
    </>
  );
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <OverlayProvider>
      <AppRuntimeProvider>
        <App />
      </AppRuntimeProvider>
    </OverlayProvider>
  </StrictMode>,
);
