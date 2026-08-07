import { useCallback, useEffect, useRef, useState } from 'react';

export type WindowBounds = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type OverlayWindowState = {
  key: string;
  manifestId: string;
  panelId: string;
  title: string;
  privileged?: boolean;
  iconUrl?: string;
  bounds: WindowBounds;
  zIndex: number;
  minimized: boolean;
};

export type AppPanelTab = {
  key: string;
  title: string;
  manifestId: string;
  panelId: string;
  privileged?: boolean;
  iconUrl?: string;
};

const STORAGE_KEY = 'glint.window-layout';
const SESSION_KEY = 'glint.window-session';
const MIN_WIDTH = 360;
const MIN_HEIGHT = 260;
const DOCK_HEIGHT = 88;

let zCounter = 10;

function maxWidth(): number {
  return typeof window !== 'undefined' ? Math.floor(window.innerWidth * 0.92) : 1600;
}

function maxHeight(): number {
  return typeof window !== 'undefined'
    ? Math.floor((window.innerHeight - DOCK_HEIGHT) * 0.88)
    : 900;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(value)));
}

function defaultBounds(index: number): WindowBounds {
  const width = clamp(Math.floor(window.innerWidth * 0.52), MIN_WIDTH, maxWidth());
  const height = clamp(Math.floor((window.innerHeight - DOCK_HEIGHT) * 0.58), MIN_HEIGHT, maxHeight());
  const x = clamp(64 + (index % 6) * 28, 8, window.innerWidth - width - 8);
  const y = clamp(48 + (index % 6) * 28, 8, window.innerHeight - DOCK_HEIGHT - height - 8);
  return { x, y, width, height };
}

type SavedLayout = Record<string, WindowBounds & { minimized?: boolean }>;

type SavedWindow = Omit<OverlayWindowState, 'zIndex'>;

type SavedSession = {
  windows: SavedWindow[];
  focusedKey: string | null;
};

function loadLayout(): SavedLayout {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as SavedLayout;
  } catch {
    return {};
  }
}

function loadSession(): SavedSession | null {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as SavedSession;
    if (!Array.isArray(parsed.windows)) return null;
    return parsed;
  } catch {
    return null;
  }
}

function saveLayout(windows: OverlayWindowState[]): void {
  try {
    const layout: SavedLayout = {};
    for (const win of windows) {
      layout[win.key] = { ...win.bounds, minimized: win.minimized };
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify(layout));
  } catch {
    // ignore quota errors
  }
}

function saveSession(windows: OverlayWindowState[], focusedKey: string | null): void {
  try {
    const session: SavedSession = {
      windows: windows.map(({ zIndex: _z, ...rest }) => rest),
      focusedKey:
        focusedKey && windows.some((w) => w.key === focusedKey)
          ? focusedKey
          : null,
    };
    localStorage.setItem(SESSION_KEY, JSON.stringify(session));
  } catch {
    // ignore quota errors
  }
}

function restoreWindows(session: SavedSession): OverlayWindowState[] {
  const layout = loadLayout();
  return session.windows.map((win, index) => {
      const savedBounds = layout[win.key];
      const bounds = savedBounds
        ? {
            x: clamp(savedBounds.x, 8, window.innerWidth - MIN_WIDTH - 8),
            y: clamp(savedBounds.y, 8, window.innerHeight - DOCK_HEIGHT - MIN_HEIGHT - 8),
            width: clamp(savedBounds.width, MIN_WIDTH, maxWidth()),
            height: clamp(savedBounds.height, MIN_HEIGHT, maxHeight()),
          }
        : win.bounds;
      zCounter += 1;
      return {
        ...win,
        bounds,
        minimized: savedBounds?.minimized ?? win.minimized,
        zIndex: zCounter + index,
      };
    });
}

function initialWindows(): OverlayWindowState[] {
  const session = loadSession();
  if (!session || session.windows.length === 0) return [];
  return restoreWindows(session);
}

function initialFocusedKey(windows: OverlayWindowState[]): string | null {
  const session = loadSession();
  if (!session?.focusedKey) return windows[0]?.key ?? null;
  return windows.some((w) => w.key === session.focusedKey)
    ? session.focusedKey
    : windows[0]?.key ?? null;
}

function loadInitialState(): { windows: OverlayWindowState[]; focusedKey: string | null } {
  const windows = initialWindows();
  return { windows, focusedKey: initialFocusedKey(windows) };
}

export function useOverlayWindows() {
  const [windows, setWindows] = useState<OverlayWindowState[]>(
    () => loadInitialState().windows,
  );
  const [focusedKey, setFocusedKey] = useState<string | null>(
    () => loadInitialState().focusedKey,
  );
  const layoutRef = useRef(loadLayout());

  const bringToFront = useCallback((key: string) => {
    zCounter += 1;
    setFocusedKey(key);
    setWindows((prev) =>
      prev.map((win) =>
        win.key === key ? { ...win, zIndex: zCounter, minimized: false } : win,
      ),
    );
  }, []);

  const openWindow = useCallback((tab: AppPanelTab) => {
    setWindows((prev) => {
      const existing = prev.find((w) => w.key === tab.key);
      if (existing) {
        zCounter += 1;
        setFocusedKey(tab.key);
        return prev.map((win) =>
          win.key === tab.key
            ? { ...win, zIndex: zCounter, minimized: false }
            : win,
        );
      }

      const saved = layoutRef.current[tab.key];
      const bounds = saved
        ? {
            x: clamp(saved.x, 8, window.innerWidth - MIN_WIDTH - 8),
            y: clamp(saved.y, 8, window.innerHeight - DOCK_HEIGHT - MIN_HEIGHT - 8),
            width: clamp(saved.width, MIN_WIDTH, maxWidth()),
            height: clamp(saved.height, MIN_HEIGHT, maxHeight()),
          }
        : defaultBounds(prev.length);

      zCounter += 1;
      setFocusedKey(tab.key);
      return [
        ...prev,
        {
          key: tab.key,
          manifestId: tab.manifestId,
          panelId: tab.panelId,
          title: tab.title,
          privileged: tab.privileged,
          iconUrl: tab.iconUrl,
          bounds,
          zIndex: zCounter,
          minimized: saved?.minimized ?? false,
        },
      ];
    });
  }, []);

  const closeWindow = useCallback((key: string) => {
    setWindows((prev) => {
      const next = prev.filter((w) => w.key !== key);
      saveLayout(next);
      return next;
    });
    setFocusedKey((prev) => (prev === key ? null : prev));
  }, []);

  const minimizeWindow = useCallback((key: string) => {
    setWindows((prev) => {
      const next = prev.map((win) =>
        win.key === key ? { ...win, minimized: true } : win,
      );
      saveLayout(next);
      return next;
    });
    setFocusedKey((prev) => (prev === key ? null : prev));
  }, []);

  const toggleWindow = useCallback(
    (tab: AppPanelTab) => {
      const existing = windows.find((w) => w.key === tab.key);
      if (!existing) {
        openWindow(tab);
        return;
      }
      if (existing.minimized || focusedKey !== tab.key) {
        bringToFront(tab.key);
        return;
      }
      minimizeWindow(tab.key);
    },
    [windows, focusedKey, openWindow, bringToFront, minimizeWindow],
  );

  const updateBounds = useCallback((key: string, bounds: WindowBounds) => {
    setWindows((prev) => {
      const next = prev.map((win) =>
        win.key === key ? { ...win, bounds } : win,
      );
      saveLayout(next);
      return next;
    });
  }, []);

  useEffect(() => {
    if (windows.length > 0) {
      saveLayout(windows);
      saveSession(windows, focusedKey);
      return;
    }
    try {
      localStorage.removeItem(SESSION_KEY);
    } catch {
      // ignore
    }
  }, [windows, focusedKey]);

  return {
    windows,
    focusedKey,
    setFocusedKey,
    openWindow,
    closeWindow,
    minimizeWindow,
    toggleWindow,
    bringToFront,
    updateBounds,
    minWidth: MIN_WIDTH,
    minHeight: MIN_HEIGHT,
    maxWidth,
    maxHeight,
  };
}
