import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { APP_DATA_DIR_NAME } from './product.js';
import { getSetting, setSetting } from './db.js';
import { projectRoot } from './paths.js';
import {
  checksumAssetName,
  parseSha256File,
  pickReleaseAssets,
  setupAssetName,
  sha256File,
  type ReleaseAsset,
} from './ota-assets.js';
import { detectInstallLayout, type InstallLayout } from './ota-layout.js';
import { isNewerVersion, parseSemver, stripV } from './ota-semver.js';
import { readProductVersion } from './ota-version.js';

export const DEFAULT_OTA_REPO = 'kewuamigo/glint-overlay';
export const AUTO_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const STARTUP_CHECK_DELAY_MS = 2500;
const USER_AGENT = 'Glint-Launcher';

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
  layout: InstallLayout;
  autoCheck: boolean;
  lastCheckAt: string | null;
  lastError: string | null;
  state: OtaState;
  available: OtaAvailable | null;
  dismissed: boolean;
  applying: boolean;
  download: OtaDownloadProgress | null;
};

type GithubRelease = {
  tag_name?: string;
  draft?: boolean;
  prerelease?: boolean;
  html_url?: string;
  body?: string | null;
  assets?: ReleaseAsset[];
};

type CachedResult = {
  available: OtaAvailable | null;
};

let quitApp: () => void = () => {};
let pushStatus: (status: OtaStatus) => void = () => {};
let runtimeState: OtaState = 'idle';
let runtimeError: string | null = null;
let runtimeAvailable: OtaAvailable | null = null;
let download: OtaDownloadProgress | null = null;
let applying = false;
let checking = false;
let startupScheduled = false;
let lastPushAt = 0;
let cachedLayout: InstallLayout | null = null;

export function configureOta(opts: {
  quit: () => void;
  push: (status: OtaStatus) => void;
}): void {
  quitApp = opts.quit;
  pushStatus = opts.push;
}

export function otaEnabled(): boolean {
  const v = (process.env.GLINT_OTA ?? '').trim().toLowerCase();
  return v !== '0' && v !== 'false' && v !== 'off';
}

export function otaRepo(): string {
  const raw = (process.env.GLINT_OTA_REPO ?? DEFAULT_OTA_REPO).trim();
  if (/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(raw)) return raw;
  return DEFAULT_OTA_REPO;
}

function updatesDir(): string {
  const base = process.env.LOCALAPPDATA || process.env.USERPROFILE || '.';
  return path.join(base, APP_DATA_DIR_NAME, 'updates');
}

function autoCheckEnabled(): boolean {
  return getSetting('otaAutoCheck') !== '0';
}

function lastCheckAt(): string | null {
  return getSetting('otaLastCheckAt');
}

function dismissedVersion(): string | null {
  return getSetting('otaDismissedVersion');
}

function dismissedAtMs(): number | null {
  const raw = getSetting('otaDismissedAt');
  if (!raw) return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

function isDismissed(version: string | null): boolean {
  if (!version) return false;
  if (dismissedVersion() !== version) return false;
  const at = dismissedAtMs();
  if (at == null) return true;
  return Date.now() - at < AUTO_CHECK_INTERVAL_MS;
}

function readCached(): CachedResult | null {
  const raw = getSetting('otaLastResult');
  if (!raw) return null;
  try {
    return JSON.parse(raw) as CachedResult;
  } catch {
    return null;
  }
}

function persistCheck(opts: {
  at: string;
  etag: string | null;
  available: OtaAvailable | null;
}): void {
  setSetting('otaLastCheckAt', opts.at);
  if (opts.etag) setSetting('otaLastEtag', opts.etag);
  setSetting('otaLastResult', JSON.stringify({ available: opts.available }));
}

function notesSummary(body: string | null | undefined): string | null {
  if (!body) return null;
  const text = body.replace(/\r\n/g, '\n').trim();
  if (!text) return null;
  const first = text.split('\n').find((line) => line.trim() && !line.startsWith('#'));
  const line = (first ?? text).trim();
  return line.length > 280 ? `${line.slice(0, 277)}…` : line;
}

function toAvailable(release: GithubRelease, tagVersion: string): OtaAvailable {
  const picked = pickReleaseAssets(release.assets ?? [], tagVersion);
  return {
    version: tagVersion,
    releaseUrl: typeof release.html_url === 'string' ? release.html_url : '',
    notes: notesSummary(release.body),
    setupAsset: Boolean(picked.setup),
    checksumAsset: Boolean(picked.checksum),
    portableAsset: Boolean(picked.portable),
    setupUrl: picked.setup?.browser_download_url ?? null,
    checksumUrl: picked.checksum?.browser_download_url ?? null,
    portableUrl: picked.portable?.browser_download_url ?? null,
  };
}

function currentLayout(): InstallLayout {
  if (cachedLayout) return cachedLayout;
  cachedLayout = detectInstallLayout(projectRoot());
  return cachedLayout;
}

function emit(force = false): void {
  const now = Date.now();
  if (!force && now - lastPushAt < 80) return;
  lastPushAt = now;
  try {
    pushStatus(getOtaStatus());
  } catch {
    // window gone
  }
}

export function getOtaStatus(): OtaStatus {
  const currentVersion = readProductVersion();
  const enabled = otaEnabled();
  const layout = currentLayout();
  let available = runtimeAvailable;
  if (!available) {
    const cached = readCached()?.available ?? null;
    if (cached && isNewerVersion(currentVersion, cached.version)) {
      available = cached;
    }
  } else if (!isNewerVersion(currentVersion, available.version)) {
    available = null;
  }

  let state: OtaState = runtimeState;
  if (!enabled) state = 'idle';
  else if (applying) state = runtimeState === 'downloading' ? 'downloading' : 'applying';
  else if (checking) state = 'checking';
  else if (runtimeError) state = 'error';
  else if (available) state = 'available';
  else if (lastCheckAt()) state = 'up-to-date';
  else state = 'idle';

  return {
    enabled,
    currentVersion,
    layout,
    autoCheck: autoCheckEnabled(),
    lastCheckAt: lastCheckAt(),
    lastError: enabled ? runtimeError : null,
    state,
    available: enabled ? available : null,
    dismissed: isDismissed(available?.version ?? null),
    applying,
    download,
  };
}

function githubHeaders(etag?: string | null): Record<string, string> {
  const headers: Record<string, string> = {
    Accept: 'application/vnd.github+json',
    'User-Agent': USER_AGENT,
    'X-GitHub-Api-Version': '2022-11-28',
  };
  if (etag) headers['If-None-Match'] = etag;
  const token = (process.env.GLINT_OTA_TOKEN ?? '').trim();
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

function downloadHeaders(accept: string): Record<string, string> {
  const headers: Record<string, string> = {
    'User-Agent': USER_AGENT,
    Accept: accept,
  };
  const token = (process.env.GLINT_OTA_TOKEN ?? '').trim();
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

function withTimeout(ms: number): AbortSignal {
  return AbortSignal.timeout(ms);
}

function httpErrorMessage(status: number): string {
  if (status === 403 || status === 429) {
    return 'GitHub rate limit reached. Try again later.';
  }
  if (status === 404) {
    return 'No published GitHub Release found.';
  }
  return `GitHub Releases check failed (HTTP ${status}).`;
}

export async function checkForUpdates(force: boolean): Promise<OtaStatus> {
  if (!otaEnabled()) {
    runtimeError = null;
    runtimeState = 'idle';
    return getOtaStatus();
  }
  if (checking) return getOtaStatus();

  const previous = lastCheckAt();
  if (!force && previous) {
    const then = Date.parse(previous);
    if (Number.isFinite(then) && Date.now() - then < AUTO_CHECK_INTERVAL_MS) {
      runtimeState = getOtaStatus().available ? 'available' : 'up-to-date';
      return getOtaStatus();
    }
  }

  checking = true;
  runtimeError = null;
  runtimeState = 'checking';
  emit(true);

  try {
    const repo = otaRepo();
    const url = `https://api.github.com/repos/${repo}/releases/latest`;
    const etag = force ? null : getSetting('otaLastEtag');
    const res = await fetch(url, {
      headers: githubHeaders(etag),
      signal: withTimeout(30_000),
    });

    if (res.status === 304) {
      const at = new Date().toISOString();
      persistCheck({
        at,
        etag,
        available: readCached()?.available ?? runtimeAvailable,
      });
      const currentVersion = readProductVersion();
      const cached = readCached()?.available ?? null;
      runtimeAvailable =
        cached && isNewerVersion(currentVersion, cached.version) ? cached : null;
      runtimeError = null;
      runtimeState = runtimeAvailable ? 'available' : 'up-to-date';
      return getOtaStatus();
    }

    if (res.status === 404) {
      throw new Error(httpErrorMessage(404));
    }

    if (!res.ok) {
      throw new Error(httpErrorMessage(res.status));
    }

    const release = (await res.json()) as GithubRelease;
    const includePrerelease =
      (process.env.GLINT_OTA_PRERELEASE ?? '').trim() === '1';
    const at = new Date().toISOString();
    const nextEtag = res.headers.get('etag');

    if (
      !release ||
      release.draft === true ||
      (release.prerelease === true && !includePrerelease)
    ) {
      runtimeAvailable = null;
      persistCheck({ at, etag: nextEtag, available: null });
      runtimeError = null;
      runtimeState = 'up-to-date';
      return getOtaStatus();
    }

    const tag = typeof release.tag_name === 'string' ? release.tag_name : '';
    const version = stripV(tag);
    if (!parseSemver(version)) {
      throw new Error(`Release tag is not semver: ${tag || '(empty)'}`);
    }

    const currentVersion = readProductVersion();
    if (!isNewerVersion(currentVersion, version)) {
      runtimeAvailable = null;
      persistCheck({ at, etag: nextEtag, available: null });
      runtimeError = null;
      runtimeState = 'up-to-date';
      return getOtaStatus();
    }

    runtimeAvailable = toAvailable(release, version);
    persistCheck({ at, etag: nextEtag, available: runtimeAvailable });
    runtimeError = null;
    runtimeState = 'available';
    return getOtaStatus();
  } catch (err) {
    runtimeError = err instanceof Error ? err.message : String(err);
    runtimeState = 'error';
    if (/rate limit/i.test(runtimeError)) {
      setSetting('otaLastCheckAt', new Date().toISOString());
    }
    return getOtaStatus();
  } finally {
    checking = false;
    emit(true);
  }
}

export function dismissUpdate(): OtaStatus {
  const status = getOtaStatus();
  const version = status.available?.version;
  if (version) {
    setSetting('otaDismissedVersion', version);
    setSetting('otaDismissedAt', String(Date.now()));
  }
  emit(true);
  return getOtaStatus();
}

export async function openReleasePage(): Promise<OtaStatus> {
  const { shell } = await import('electron');
  const status = getOtaStatus();
  const url =
    status.available?.releaseUrl ||
    `https://github.com/${otaRepo()}/releases`;
  await shell.openExternal(url);
  return status;
}

async function downloadFile(
  url: string,
  dest: string,
  onProgress: (received: number, total: number | null) => void,
): Promise<void> {
  const res = await fetch(url, {
    headers: downloadHeaders('application/octet-stream'),
    redirect: 'follow',
    signal: withTimeout(15 * 60_000),
  });
  if (!res.ok) {
    throw new Error(`Download failed (HTTP ${res.status}).`);
  }
  const totalHeader = res.headers.get('content-length');
  const total = totalHeader ? Number(totalHeader) : null;
  const length = total && Number.isFinite(total) && total > 0 ? total : null;
  if (!res.body) throw new Error('Download failed (empty body).');

  const tmp = `${dest}.part`;
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  const file = fs.createWriteStream(tmp);
  const reader = res.body.getReader();
  let received = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      received += value.byteLength;
      if (!file.write(Buffer.from(value))) {
        await new Promise<void>((resolve) => file.once('drain', resolve));
      }
      onProgress(received, length);
    }
    await new Promise<void>((resolve, reject) => {
      file.end(() => resolve());
      file.once('error', reject);
    });
    fs.renameSync(tmp, dest);
  } catch (err) {
    try {
      file.destroy();
    } catch {
      /* ignore */
    }
    try {
      fs.unlinkSync(tmp);
    } catch {
      /* ignore */
    }
    throw err;
  }
}

async function verifyDownloaded(
  filePath: string,
  available: OtaAvailable,
): Promise<void> {
  if (!available.checksumAsset || !available.checksumUrl) return;
  const res = await fetch(available.checksumUrl, {
    headers: downloadHeaders('text/plain'),
    redirect: 'follow',
    signal: withTimeout(30_000),
  });
  if (!res.ok) {
    throw new Error(`Checksum download failed (HTTP ${res.status}).`);
  }
  const body = await res.text();
  const expected = parseSha256File(body, setupAssetName(available.version));
  if (!expected) {
    throw new Error('Checksum file did not list the Setup installer.');
  }
  const actual = await sha256File(filePath);
  if (actual !== expected) {
    throw new Error('Setup installer checksum mismatch.');
  }
}

function launchSetupAfterQuit(setupPath: string): Promise<void> {
  // Detached PowerShell waits for this Electron PID to exit, then ShellExecutes
  // Setup so UAC / Inno run against a dead launcher tree (not locked DLLs).
  const pid = process.pid;
  const script = [
    `$p = Get-Process -Id ${pid} -ErrorAction SilentlyContinue`,
    'if ($p) { Wait-Process -Id $p.Id -ErrorAction SilentlyContinue }',
    `Start-Process -FilePath ${JSON.stringify(setupPath)} -ArgumentList '/SILENT','/NORESTART'`,
  ].join('; ');
  return new Promise((resolve, reject) => {
    const child = spawn(
      'powershell.exe',
      ['-NoProfile', '-WindowStyle', 'Hidden', '-Command', script],
      { detached: true, stdio: 'ignore', windowsHide: true },
    );
    child.once('error', (err) => {
      reject(err);
    });
    child.once('spawn', () => {
      child.unref();
      resolve();
    });
  });
}

export async function applyUpdate(): Promise<OtaStatus> {
  if (!otaEnabled()) {
    throw new Error('OTA is disabled (GLINT_OTA=0).');
  }
  if (applying) return getOtaStatus();

  const status = getOtaStatus();
  const available = status.available;
  if (!available) {
    throw new Error('No update is available.');
  }

  if (status.layout !== 'inno') {
    await openReleasePage();
    return getOtaStatus();
  }

  if (!available.setupAsset || !available.setupUrl) {
    throw new Error(
      'This release has no Setup installer. Open the GitHub Release to download it.',
    );
  }

  applying = true;
  runtimeError = null;
  runtimeState = 'downloading';
  download = { received: 0, total: null };
  emit(true);

  const dest = path.join(updatesDir(), setupAssetName(available.version));
  try {
    let reuse = false;
    if (fs.existsSync(dest) && available.checksumAsset) {
      try {
        await verifyDownloaded(dest, available);
        reuse = true;
      } catch {
        try {
          fs.unlinkSync(dest);
        } catch {
          /* ignore */
        }
      }
    }

    if (!reuse) {
      await downloadFile(available.setupUrl, dest, (received, total) => {
        download = { received, total };
        runtimeState = 'downloading';
        emit(false);
      });
      await verifyDownloaded(dest, available);
    }

    runtimeState = 'applying';
    download = null;
    emit(true);
    await launchSetupAfterQuit(dest);
    quitApp();
    return getOtaStatus();
  } catch (err) {
    runtimeError = err instanceof Error ? err.message : String(err);
    runtimeState = 'error';
    applying = false;
    download = null;
    emit(true);
    throw err;
  }
}

export async function maybeAutoCheck(): Promise<void> {
  if (!otaEnabled()) return;
  if (!autoCheckEnabled()) return;
  await checkForUpdates(false);
}

export function scheduleOtaStartupCheck(): void {
  if (startupScheduled) return;
  startupScheduled = true;
  setTimeout(() => {
    void maybeAutoCheck().catch(() => {
      /* fail soft */
    });
  }, STARTUP_CHECK_DELAY_MS);
}
