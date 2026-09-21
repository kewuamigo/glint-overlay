import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';
import type { MetricsSnapshot } from '@glint/plugin-sdk';
import { defaultMetrics } from '@glint/plugin-sdk';
import { hostInvoke } from '@glint/overlay-bridge';

export interface OverlayState {
  connected: boolean;
  targetPid: number | null;
  gameName: string | null;
  playtimeSeconds: number | null;
  attachedAtMs: number | null;
  overlayOpen: boolean;
  panelPins: Record<string, boolean>;
  anyPinned: boolean;
  metrics: MetricsSnapshot;
  metricsHistory: { native: number[]; generated: number[] };
}

interface OverlayContextValue extends OverlayState {
  setMetrics: (metrics: MetricsSnapshot) => void;
  setConnected: (
    connected: boolean,
    pid?: number,
    session?: { gameName?: unknown; playtimeSeconds?: unknown },
  ) => void;
  setOverlayOpen: (open: boolean) => void;
  setPanelPin: (panelKey: string, pinned: boolean) => void;
}

const OverlayContext = createContext<OverlayContextValue | null>(null);

export function OverlayProvider({
  children,
  initialMetrics,
}: {
  children: ReactNode;
  initialMetrics?: MetricsSnapshot;
}) {
  const [connected, setConnectedState] = useState(false);
  const [targetPid, setTargetPid] = useState<number | null>(null);
  const [gameName, setGameName] = useState<string | null>(null);
  const [playtimeSeconds, setPlaytimeSeconds] = useState<number | null>(null);
  const [attachedAtMs, setAttachedAtMs] = useState<number | null>(null);
  const [overlayOpen, setOverlayOpenState] = useState(false);
  const [panelPins, setPanelPinsState] = useState<Record<string, boolean>>({});

  const anyPinned = useMemo(
    () => Object.values(panelPins).some(Boolean),
    [panelPins],
  );

  const [metrics, setMetricsState] = useState<MetricsSnapshot>(
    initialMetrics ?? defaultMetrics,
  );
  const [metricsHistory, setMetricsHistory] = useState<{
    native: number[];
    generated: number[];
  }>({ native: [], generated: [] });

  // All setters that escape into child component dependency arrays must be
  // stable references (useCallback). Without stability, AppHost's effect
  // would re-run on every render and perpetually re-apply defaultPinned,
  // making it impossible to un-pin a panel during a session.
  const setPanelPin = useCallback((panelKey: string, pinned: boolean) => {
    setPanelPinsState((prev) => ({ ...prev, [panelKey]: pinned }));
    void hostInvoke('panel.setPinned', [panelKey, pinned]);
  }, []);

  const setMetrics = useCallback((next: MetricsSnapshot) => {
    if (!next || typeof next.nativeFps !== 'number') return;
    setMetricsState(next);
    setMetricsHistory((prev) => ({
      native: [...prev.native.slice(-89), next.nativeFps],
      generated: [...prev.generated.slice(-89), next.generatedFps],
    }));
  }, []);

  const setConnected = useCallback(
    (
      value: boolean,
      pid?: number,
      session?: { gameName?: unknown; playtimeSeconds?: unknown; attachedAtMs?: unknown },
    ) => {
      setConnectedState(value);
      setTargetPid(pid ?? null);
      const seconds = session?.playtimeSeconds;
      setGameName(typeof session?.gameName === 'string' ? session.gameName : null);
      setPlaytimeSeconds(typeof seconds === 'number' ? seconds : null);
      const attached = session?.attachedAtMs;
      setAttachedAtMs(typeof attached === 'number' ? attached : null);
    },
    [],
  );

  const setOverlayOpen = useCallback((open: boolean) => {
    setOverlayOpenState(open);
  }, []);

  const value: OverlayContextValue = {
    connected,
    targetPid,
    gameName,
    playtimeSeconds,
    attachedAtMs,
    overlayOpen,
    panelPins,
    anyPinned,
    metrics,
    metricsHistory,
    setMetrics,
    setConnected,
    setOverlayOpen,
    setPanelPin,
  };

  return (
    <OverlayContext.Provider value={value}>{children}</OverlayContext.Provider>
  );
}

export function useOverlay() {
  const ctx = useContext(OverlayContext);
  if (!ctx) throw new Error('useOverlay must be used within OverlayProvider');
  return ctx;
}
