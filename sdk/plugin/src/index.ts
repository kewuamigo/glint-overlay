import type { FC } from 'react';

export type PluginPermission =
  | 'metrics'
  | 'storage'
  | 'db'
  | 'fs:read'
  | 'fs:write'
  | 'fs:pick'
  | 'game:process'
  | 'game:memory:read'
  | 'game:memory:write'
  | 'game:invoke';

export type HudSlot =
  | 'top-left'
  | 'top-right'
  | 'bottom-left'
  | 'bottom-right'
  | 'top-center';

export interface PluginPanelManifest {
  id: string;
  title: string;
  /** Relative path to icon image (png/svg) inside the app bundle. */
  icon?: string;
  pinnable?: boolean;
  defaultPinned?: boolean;
  hudSlot?: HudSlot;
}

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  entry: string;
  /** Built-in core app shipped with the platform; cannot be removed by users. */
  builtin?: boolean;
  /** Privileged app (e.g. browser webview); may use host-only SDK surfaces. */
  privileged?: boolean;
  permissions?: PluginPermission[];
  panels: PluginPanelManifest[];
}

export type PluginRenderMode = 'hud' | 'full';

/** Which frame-generation technology is producing the extra frames. */
export type FrameGenKind = 'none' | 'dlss' | 'fsr' | 'xefg' | 'afmf' | 'unknown';

/** How the native/generated frame split was derived. */
export type FrameSplitSource = 'driver' | 'hook' | 'etw';

export type MetricsDetailLevel = 'classic' | 'util' | 'full';

export interface MetricsTiles {
  fps?: boolean;
  cpu?: boolean;
  gpu?: boolean;
  ram?: boolean;
  vram?: boolean;
  graph?: boolean;
}

export interface MetricsPrefs {
  detailLevel: MetricsDetailLevel;
  tiles: MetricsTiles;
}

export interface MetricsSnapshot {
  nativeFps: number;
  generatedFps: number;
  frameGenActive: boolean;
  frameGenRatio: number;
  frameGenKind: FrameGenKind;
  frameSplitSource: FrameSplitSource;
  nativeMin: number;
  nativeMax: number;
  generatedMin: number;
  generatedMax: number;
  nativeFrameTimeMs: number;
  displayFrameTimeMs: number;
  gpuUtil?: number;
  cpuUtil?: number;
  ramUsedMb?: number;
  ramTotalMb?: number;
  vramDedicatedMb?: number;
  vramSharedMb?: number;
  detailLevel?: MetricsDetailLevel;
  tiles?: MetricsTiles;
}

export const defaultMetrics: MetricsSnapshot = {
  nativeFps: 0,
  generatedFps: 0,
  frameGenActive: false,
  frameGenRatio: 0,
  frameGenKind: 'none',
  frameSplitSource: 'etw',
  nativeMin: 0,
  nativeMax: 0,
  generatedMin: 0,
  generatedMax: 0,
  nativeFrameTimeMs: 0,
  displayFrameTimeMs: 0,
};

/** Steam-style display label for the frame-generation source. */
export function frameGenLabel(kind: FrameGenKind): string {
  switch (kind) {
    case 'dlss':
      return 'DLSS';
    case 'fsr':
      return 'FSR';
    case 'xefg':
      return 'XeSS FG';
    case 'afmf':
      return 'AFMF';
    case 'unknown':
      return 'FG';
    default:
      return '';
  }
}

export interface ProcessInfo {
  pid: number;
  exePath: string;
  exeName: string;
  windowTitle: string | null;
}

export interface SaveLocation {
  path: string;
  tags: Array<'save' | 'config' | 'settings'>;
  exists: boolean;
}

export interface SaveManifestEntry {
  title: string;
  steamId?: number;
  files: Record<string, { tags?: string[] }>;
  notes?: string[];
}

export interface StorageAPI {
  get<T = unknown>(key: string): Promise<T | null>;
  set<T = unknown>(key: string, value: T): Promise<void>;
  remove(key: string): Promise<void>;
}

/** Host-provided SQLite, isolated to this app's data/app.db. */
export interface DatabaseAPI {
  exec(sql: string): Promise<void>;
  run(
    sql: string,
    params?: unknown[],
  ): Promise<{ changes: number; lastInsertRowid: number | bigint }>;
  get<T = Record<string, unknown>>(
    sql: string,
    params?: unknown[],
  ): Promise<T | null>;
  all<T = Record<string, unknown>>(
    sql: string,
    params?: unknown[],
  ): Promise<T[]>;
}

export interface FileSystemAPI {
  readText(path: string): Promise<string>;
  readBytes(path: string): Promise<Uint8Array>;
  writeText(path: string, content: string): Promise<void>;
  writeBytes(path: string, data: Uint8Array): Promise<void>;
  exists(path: string): Promise<boolean>;
  isDirectory(path: string): Promise<boolean>;
  listDir(path: string): Promise<string[]>;
  pickFolder(): Promise<string | null>;
  pickFile(): Promise<string | null>;
}

export interface SaveAPI {
  getGameDir(): Promise<string>;
  getManifestEntry(): Promise<SaveManifestEntry | null>;
  getSaveLocations(): Promise<SaveLocation[]>;
  findGame(query: string): Promise<SaveManifestEntry[]>;
  updateManifest(): Promise<{ updated: boolean; gameCount: number }>;
}

export interface MemoryAPI {
  read(address: bigint, size: number): Promise<Uint8Array>;
  write(address: bigint, data: Uint8Array): Promise<void>;
  readI32(address: bigint): Promise<number>;
  readU32(address: bigint): Promise<number>;
  readF32(address: bigint): Promise<number>;
  readU64(address: bigint): Promise<bigint>;
  writeI32(address: bigint, value: number): Promise<void>;
  writeF32(address: bigint, value: number): Promise<void>;
  scanPattern(module: string, pattern: string): Promise<bigint[]>;
  resolvePointerChain(base: bigint, offsets: number[]): Promise<bigint>;
}

export interface ModuleInfo {
  name: string;
  base: bigint;
  size: number;
}

export interface ModuleAPI {
  list(): Promise<ModuleInfo[]>;
  getBase(name: string): Promise<bigint>;
  getExport(module: string, exportName: string): Promise<bigint>;
}

const phase3Stub = (): never => {
  throw new Error('game:memory API is not available until Phase 3');
};

export const memoryApiStub: MemoryAPI = {
  read: phase3Stub,
  write: phase3Stub,
  readI32: phase3Stub,
  readU32: phase3Stub,
  readF32: phase3Stub,
  readU64: phase3Stub,
  writeI32: phase3Stub,
  writeF32: phase3Stub,
  scanPattern: phase3Stub,
  resolvePointerChain: phase3Stub,
};

export const moduleApiStub: ModuleAPI = {
  list: phase3Stub,
  getBase: phase3Stub,
  getExport: phase3Stub,
};

export interface PluginProps {
  api: PluginAPI;
  mode: PluginRenderMode;
}

export type PluginComponent = FC<PluginProps>;

export function hasPermission(
  manifest: PluginManifest,
  permission: PluginPermission,
): boolean {
  return manifest.permissions?.includes(permission) ?? false;
}

export interface PluginManifestSummary {
  id: string;
  name: string;
  version: string;
  entry: string;
  /** Opaque native overlay client exe (relative to app root or absolute). */
  native?: string;
  builtin?: boolean;
  privileged?: boolean;
  permissions: PluginPermission[];
  panels: PluginPanelManifest[];
  bundleUrl: string;
  /** Resolved glint-plugin:// URL for the app icon, if declared in manifest. */
  iconUrl?: string;
}

export type OverlayModeName = 'hidden' | 'hud_pinned' | 'interactive';

export interface WindowManagerSnapshot {
  mode: OverlayModeName;
  hudPinned: boolean;
  panelPins: Record<string, boolean>;
  gameWindowId: number | null;
}

export interface NativeOverlayAPI {
  getGameWindowId(): Promise<number | null>;
  setPosition(x: number, y: number): Promise<void>;
  setAnchor(x: number, y: number): Promise<void>;
  setMargin(top: number, right: number, bottom: number, left: number): Promise<void>;
  listenInput(cursor: boolean, keyboard: boolean): Promise<void>;
  blockInput(block: boolean): Promise<void>;
  setBlockingCursor(cursor?: string): Promise<void>;
}

export interface NativeMetricsAPI {
  getSnapshot(): Promise<MetricsSnapshot>;
  getPrefs(): Promise<MetricsPrefs>;
  setPrefs(prefs: Partial<MetricsPrefs>): Promise<MetricsPrefs>;
}

export interface NativeWindowAPI {
  getSnapshot(): Promise<WindowManagerSnapshot>;
  toggleInteractive(): Promise<OverlayModeName>;
  setMode(mode: OverlayModeName): Promise<OverlayModeName>;
}

export interface BrowserContentRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface BrowserAPI {
  focus(): Promise<void>;
  blur(): Promise<void>;
  setContentRect(rect: BrowserContentRect | null): Promise<void>;
}

export interface NativeAPI {
  overlay: NativeOverlayAPI;
  metrics: NativeMetricsAPI;
  window: NativeWindowAPI;
}

export interface PluginAPI {
  pluginId: string;
  panelId: string;
  panels: {
    setPinned(pinned: boolean): void;
    isPinned(): boolean;
  };
  overlay: {
    open(): void;
    close(): void;
    isOpen(): boolean;
  };
  native: NativeAPI;
  browser: BrowserAPI;
  game: {
    targetPid: number | null;
    getProcessInfo(): Promise<ProcessInfo>;
    onMetrics(cb: (m: MetricsSnapshot) => void): () => void;
    modules: ModuleAPI;
    memory: MemoryAPI;
    saves: SaveAPI;
  };
  fs: FileSystemAPI;
  storage: StorageAPI;
  db: DatabaseAPI;
}
