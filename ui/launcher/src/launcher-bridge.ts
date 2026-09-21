export type LauncherTab = 'home' | 'library' | 'store' | 'achievements' | 'settings';

export type GameArt = {
  icon?: string;
  grid?: string;
  hero?: string;
  logo?: string;
  iconMime?: string;
  gridMime?: string;
  heroMime?: string;
  logoMime?: string;
};

export type ArtSlot = 'icon' | 'grid' | 'hero' | 'logo';

export type SgdbAsset = {
  id: number;
  url: string;
  thumb: string;
  mime: string;
  style: string;
  animated: boolean;
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

export type OtaState =
  | 'idle'
  | 'checking'
  | 'up-to-date'
  | 'available'
  | 'downloading'
  | 'applying'
  | 'error';

export type OtaAvailable = {
  version: string;
  releaseUrl: string;
  notes: string | null;
  setupAsset: boolean;
  checksumAsset: boolean;
  portableAsset: boolean;
  setupUrl: string | null;
  checksumUrl: string | null;
  portableUrl: string | null;
};

export type OtaDownloadProgress = {
  received: number;
  total: number | null;
};

export type OtaStatus = {
  enabled: boolean;
  currentVersion: string;
  layout: 'inno' | 'portable';
  autoCheck: boolean;
  lastCheckAt: string | null;
  lastError: string | null;
  state: OtaState;
  available: OtaAvailable | null;
  dismissed: boolean;
  applying: boolean;
  download: OtaDownloadProgress | null;
};

/** Mirrors `@glint/achievements-core` PrepareState (UI has no package dep). */
export type PrepareStatus = 'pending' | 'ready' | 'failed' | 'skipped';

export type PrepareState = {
  status: PrepareStatus;
  error?: string;
  steamAppId?: string;
  updatedAt: number;
};

/** Same rule as achievements-core `prepareBlocksLaunch`. */
export function prepareBlocksLaunch(
  status: PrepareState | null | undefined,
): boolean {
  if (!status) return true;
  return status.status === 'pending' || status.status === 'failed';
}

export type SyncRevisionInfo = {
  gameId: string;
  revisionId: string;
  createdAt: string;
  contentHash: string;
};

export type LauncherBridge = {
  invoke<T = unknown>(method: string, args?: unknown[]): Promise<T>;
  onSyncStatus?: (cb: (payload: SyncStatusPush) => void) => () => void;
  onOtaStatus?: (cb: (payload: OtaStatus) => void) => () => void;
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

export function onOtaStatus(cb: (payload: OtaStatus) => void): () => void {
  return window.__goLauncher?.onOtaStatus?.(cb) ?? (() => {});
}
