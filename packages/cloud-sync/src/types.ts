/** One packed sync revision on disk: manifest.json + saves/ + achievements.json */

export interface SyncSaveRootEntry {
  /** Directory name under `saves/` in the revision */
  name: string;
  /** Absolute local path this root was packed from (for restore) */
  sourcePath: string;
  /** SHA-256 of packed files under this root; null if root was empty/missing */
  contentHash: string | null;
}

export interface SyncRevisionManifest {
  version: 1;
  gameId: string;
  revisionId: string;
  /** ISO-8601 timestamp when the revision was packed */
  createdAt: string;
  /** Aggregate SHA-256 over packed saves + achievements payloads */
  contentHash: string;
  saves: {
    /** Relative directory in the revision (`saves`) */
    path: 'saves';
    roots: SyncSaveRootEntry[];
    contentHash: string | null;
  };
  achievements: {
    /** Relative file in the revision when included */
    path: 'achievements.json' | null;
    contentHash: string | null;
  };
}

export interface SyncRevisionInfo {
  gameId: string;
  revisionId: string;
  createdAt: string;
  contentHash: string;
}

/** Input for pack — callers resolve Ludusavi paths via `@glint/save-manifest` first. */
export interface PackRevisionInput {
  gameId: string;
  /** Absolute save location paths (already resolved). Missing/empty roots are skipped. */
  saveLocations?: string[];
  /** Achievements snapshot; omit for saves-only. */
  achievements?: unknown;
  /** Override revision id / createdAt (tests). */
  revisionId?: string;
  createdAt?: Date;
}

export interface PackRevisionResult {
  revisionDir: string;
  manifest: SyncRevisionManifest;
}

export interface UnpackRevisionResult {
  manifest: SyncRevisionManifest;
  /** Parsed achievements.json, or null if the revision had none */
  achievements: unknown | null;
  /** Roots restored (name → destination path) */
  restoredSaves: Array<{ name: string; destPath: string }>;
}

export interface SelfHostStorageConfig {
  provider: 'selfhost';
  baseUrl: string;
  bearerToken: string;
}

/**
 * Google Drive (v1): paste a short-lived `accessToken` from OAuth Playground /
 * a one-shot script. Browser OAuth + automatic refresh are deferred.
 * `refreshToken` is optional and unused in v1 (do not assume refresh works).
 */
export interface DriveStorageConfig {
  provider: 'drive';
  /** Paste a current Drive API access token (Bearer). */
  accessToken: string;
  /** Optional; unused in v1 — no token refresh without OAuth client credentials. */
  refreshToken?: string;
  /** Dedicated Drive folder for Glint sync revisions */
  folderId?: string;
}

export interface FtpStorageConfig {
  provider: 'ftp';
  host: string;
  user: string;
  password: string;
  basePath: string;
  /** true = FTPS; false = plain FTP (adapters should warn) */
  secure: boolean;
  port?: number;
}

export type CloudStorageConfig =
  | SelfHostStorageConfig
  | DriveStorageConfig
  | FtpStorageConfig;
