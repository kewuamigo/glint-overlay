export type LauncherTab = 'home' | 'library' | 'store' | 'achievements' | 'settings';

export type GameArt = {
  icon?: string;
  grid?: string;
  hero?: string;
};

export type ScannedGame = {
  id: string;
  name: string;
  source: string;
  exe: string;
  install_path: string;
  running: boolean;
  pid?: number;
  playtime_hours: number | null;
};

export type Profile = {
  username: string;
  display_name: string;
  status: string;
};

export type StoreApp = {
  id: string;
  name: string;
  version: string;
  description: string;
  iconUrl?: string;
  downloadUrl: string;
};

export type StoreCatalog = {
  version: number;
  apps: StoreApp[];
};

export type InstalledApp = {
  id: string;
  name: string;
  version: string;
  icon?: string;
};

/** Mirrors `@glint/cloud-sync` CloudStorageConfig (UI has no package dep). */
export type CloudStorageConfig =
  | { provider: 'selfhost'; baseUrl: string; bearerToken: string }
  | {
      provider: 'ftp';
      host: string;
      user: string;
      password: string;
      basePath: string;
      secure: boolean;
      port?: number;
    }
  | {
      provider: 'drive';
      accessToken: string;
      refreshToken?: string;
      folderId?: string;
    };

export type CloudStorageState = {
  enabled: boolean;
  config: CloudStorageConfig | null;
};

export type GameSyncStatus =
  | 'idle'
  | 'syncing'
  | 'ok'
  | 'error'
  | 'conflict'
  | 'skipped';

export type GameSyncState = {
  lastSuccessfulSync: string | null;
  conflictPending: boolean;
  lastError: string | null;
  lastStatus: GameSyncStatus;
};

export type SyncStatusPush = {
  gameId: string;
  status: GameSyncState;
  phase?: string;
};

export type CloudSyncStatusResponse = {
  enabled: boolean;
  configured: boolean;
  games: Record<string, GameSyncState>;
  game?: GameSyncState;
};

export type SyncRevisionInfo = {
  gameId: string;
  revisionId: string;
  createdAt: string;
  contentHash: string;
};

export type LauncherBridge = {
  invoke<T = unknown>(method: string, args?: unknown[]): Promise<T>;
  onSyncStatus?: (cb: (payload: SyncStatusPush) => void) => () => void;
};

declare global {
  interface Window {
    __goLauncher?: LauncherBridge;
  }
}

export function launcherInvoke<T>(
  method: string,
  args: unknown[] = [],
): Promise<T> {
  const bridge = window.__goLauncher;
  if (!bridge) {
    return Promise.reject(new Error('launcher bridge unavailable'));
  }
  return bridge.invoke<T>(method, args);
}

export function onSyncStatus(
  cb: (payload: SyncStatusPush) => void,
): () => void {
  return window.__goLauncher?.onSyncStatus?.(cb) ?? (() => {});
}
