import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { stripV } from './ota-semver.js';

export type ReleaseAsset = {
  name: string;
  browser_download_url: string;
};

export type PickedAssets = {
  setup: ReleaseAsset | null;
  checksum: ReleaseAsset | null;
  portable: ReleaseAsset | null;
};

export function setupAssetName(version: string): string {
  return `Glint-Setup-${stripV(version)}.exe`;
}

export function checksumAssetName(version: string): string {
  return `Glint-Setup-${stripV(version)}.exe.sha256`;
}

export function portableAssetName(version: string): string {
  return `Glint-portable-${stripV(version)}.zip`;
}

/**
 * Pick Setup / checksum / portable assets by exact release filename.
 * Never substitutes a different asset as Setup.
 */
export function pickReleaseAssets(
  assets: ReleaseAsset[],
  version: string,
): PickedAssets {
  const ver = stripV(version);
  const setupName = setupAssetName(ver);
  const checksumName = checksumAssetName(ver);
  const portableName = portableAssetName(ver);
  let setup: ReleaseAsset | null = null;
  let checksum: ReleaseAsset | null = null;
  let portable: ReleaseAsset | null = null;
  for (const asset of assets) {
    if (asset.name === setupName) setup = asset;
    else if (asset.name === checksumName || asset.name === 'SHA256SUMS') {
      if (asset.name === checksumName || !checksum) checksum = asset;
    } else if (asset.name === portableName) portable = asset;
  }
  return { setup, checksum, portable };
}

/**
 * Parse a `.sha256` sidecar or `SHA256SUMS` listing.
 * Accepts GNU coreutils (`<hash>  file` / `<hash> *file`) or a bare 64-hex digest.
 */
export function parseSha256File(
  content: string,
  filename: string,
): string | null {
  const want = filename.replace(/\\/g, '/').split('/').pop() ?? filename;
  const hex = /^[0-9a-f]{64}$/i;
  let bare: string | null = null;
  for (const raw of content.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = /^([0-9a-f]{64})\s+\*?(.+)$/i.exec(line);
    if (m) {
      const listed = m[2].trim().replace(/^\.\//, '').replace(/\\/g, '/');
      const listedBase = listed.split('/').pop() ?? listed;
      if (listed === want || listedBase === want) {
        return m[1].toLowerCase();
      }
      continue;
    }
    if (hex.test(line)) {
      bare = bare === null ? line.toLowerCase() : 'ambiguous';
    }
  }
  return bare === 'ambiguous' ? null : bare;
}

export async function sha256File(filePath: string): Promise<string> {
  const hash = createHash('sha256');
  const stream = createReadStream(filePath);
  for await (const chunk of stream) {
    hash.update(chunk);
  }
  return hash.digest('hex');
}
