import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import type {
  PluginComponent,
  PluginManifestSummary,
} from '@glint/plugin-sdk';

export interface LoadedApp {
  manifest: PluginManifestSummary;
  Component: PluginComponent;
}

interface AppRuntimeContextValue {
  manifests: PluginManifestSummary[];
  setManifests: (manifests: PluginManifestSummary[]) => void;
  loadedApps: LoadedApp[];
  setLoadedApps: (apps: LoadedApp[]) => void;
  loadErrors: Record<string, string>;
  setLoadError: (appId: string, message: string | null) => void;
}

const AppRuntimeContext = createContext<AppRuntimeContextValue | null>(null);

export function AppRuntimeProvider({ children }: { children: ReactNode }) {
  const [manifests, setManifestsState] = useState<PluginManifestSummary[]>([]);
  const [loadedApps, setLoadedApps] = useState<LoadedApp[]>([]);
  const [loadErrors, setLoadErrors] = useState<Record<string, string>>({});

  const setManifests = useCallback((next: PluginManifestSummary[]) => {
    setManifestsState(next);
  }, []);

  const setLoadError = useCallback((appId: string, message: string | null) => {
    setLoadErrors((prev) => {
      if (!message) {
        const { [appId]: _, ...rest } = prev;
        return rest;
      }
      return { ...prev, [appId]: message };
    });
  }, []);

  const value = useMemo(
    () => ({
      manifests,
      setManifests,
      loadedApps,
      setLoadedApps,
      loadErrors,
      setLoadError,
    }),
    [manifests, setManifests, loadedApps, loadErrors, setLoadError],
  );

  return (
    <AppRuntimeContext.Provider value={value}>{children}</AppRuntimeContext.Provider>
  );
}

export function useAppRuntime() {
  const ctx = useContext(AppRuntimeContext);
  if (!ctx) {
    throw new Error('useAppRuntime must be used within AppRuntimeProvider');
  }
  return ctx;
}
