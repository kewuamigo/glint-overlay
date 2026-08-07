import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { z } from 'zod';
import { appsRoot } from './apps-scan.js';
import { findCatalogApp, loadCatalog } from './catalog.js';
import { projectRoot } from './paths.js';

const manifestSchema = z.object({
  id: z.string().regex(/^[a-z0-9-]+$/),
  name: z.string().min(1),
  version: z.string().min(1),
  entry: z.string().min(1),
});

/** Minimal ZIP extract (store + deflate). No powershell / Expand-Archive. */
function extractZipSync(zipPath: string, destDir: string): void {
  const buf = fs.readFileSync(zipPath);
  let eocd = -1;
  for (let i = Math.max(0, buf.length - 22 - 65_535); i <= buf.length - 22; i++) {
    if (buf.readUInt32LE(i) === 0x06054b50) eocd = i;
  }
  if (eocd < 0) throw new Error('invalid zip: end of central directory not found');

  const entries = buf.readUInt16LE(eocd + 10);
  let cd = buf.readUInt32LE(eocd + 16);
  for (let n = 0; n < entries; n++) {
    if (buf.readUInt32LE(cd) !== 0x02014b50) {
      throw new Error('invalid zip: central directory signature');
    }
    const method = buf.readUInt16LE(cd + 10);
    const compSize = buf.readUInt32LE(cd + 20);
    const nameLen = buf.readUInt16LE(cd + 28);
    const extraLen = buf.readUInt16LE(cd + 30);
    const commentLen = buf.readUInt16LE(cd + 32);
    const localOff = buf.readUInt32LE(cd + 42);
    const name = buf.subarray(cd + 46, cd + 46 + nameLen).toString('utf8');
    cd += 46 + nameLen + extraLen + commentLen;

    const root = path.resolve(destDir) + path.sep;
    const outPath = path.resolve(destDir, name);
    if (outPath !== path.resolve(destDir) && !outPath.startsWith(root)) {
      throw new Error(`zip entry escapes destination: ${name}`);
    }
    if (name.endsWith('/') || name.endsWith('\\')) {
      fs.mkdirSync(outPath, { recursive: true });
      continue;
    }
    if (buf.readUInt32LE(localOff) !== 0x04034b50) {
      throw new Error(`invalid zip local header: ${name}`);
    }
    const lhNameLen = buf.readUInt16LE(localOff + 26);
    const lhExtraLen = buf.readUInt16LE(localOff + 28);
    const dataStart = localOff + 30 + lhNameLen + lhExtraLen;
    const compressed = buf.subarray(dataStart, dataStart + compSize);
    let data: Buffer;
    if (method === 0) data = Buffer.from(compressed);
    else if (method === 8) data = zlib.inflateRawSync(compressed);
    else throw new Error(`unsupported zip compression method ${method} in ${name}`);

    fs.mkdirSync(path.dirname(outPath), { recursive: true });
    fs.writeFileSync(outPath, data);
  }
}

export async function installStoreApp(id: string): Promise<void> {
  await loadCatalog();
  const app = findCatalogApp(id);
  if (!app) {
    throw new Error(`unknown store app: ${id}`);
  }

  const dest = path.join(appsRoot(), id);
  if (app.downloadUrl.startsWith('repo:')) {
    const rel = app.downloadUrl.slice('repo:'.length).replace(/^\//, '');
    const src = path.join(projectRoot(), rel);
    if (!fs.existsSync(src)) {
      throw new Error(`repo source not found: ${src}`);
    }
    fs.rmSync(dest, { recursive: true, force: true });
    copyDirSync(src, dest);
    validateInstall(dest, id);
    return;
  }

  const res = await fetch(app.downloadUrl);
  if (!res.ok) {
    throw new Error(`download failed: ${res.status}`);
  }
  const buffer = Buffer.from(await res.arrayBuffer());
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'go-store-'));
  const zipPath = path.join(tmpDir, `${id}.zip`);
  fs.writeFileSync(zipPath, buffer);
  const extractDir = path.join(tmpDir, 'extract');
  fs.mkdirSync(extractDir, { recursive: true });
  extractZipSync(zipPath, extractDir);

  const payload = resolvePayloadDir(extractDir, id);
  fs.rmSync(dest, { recursive: true, force: true });
  copyDirSync(payload, dest);
  fs.rmSync(tmpDir, { recursive: true, force: true });
  validateInstall(dest, id);
}

function resolvePayloadDir(extractDir: string, id: string): string {
  const nested = path.join(extractDir, id);
  if (fs.existsSync(path.join(nested, 'manifest.json'))) {
    return nested;
  }
  if (fs.existsSync(path.join(extractDir, 'manifest.json'))) {
    return extractDir;
  }
  throw new Error('downloaded archive missing manifest.json');
}

function validateInstall(dir: string, expectedId: string): void {
  const manifestPath = path.join(dir, 'manifest.json');
  if (!fs.existsSync(manifestPath)) {
    throw new Error('manifest.json missing after install');
  }
  const parsed = manifestSchema.parse(
    JSON.parse(fs.readFileSync(manifestPath, 'utf8')),
  );
  if (parsed.id !== expectedId) {
    throw new Error(`manifest id mismatch: expected ${expectedId}, got ${parsed.id}`);
  }
  const entryPath = path.join(dir, parsed.entry.replace(/^\.\//, ''));
  if (!fs.existsSync(entryPath)) {
    throw new Error(`entry file missing: ${parsed.entry}`);
  }
}

function copyDirSync(src: string, dest: string): void {
  fs.mkdirSync(dest, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const from = path.join(src, entry.name);
    const to = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      copyDirSync(from, to);
    } else {
      fs.copyFileSync(from, to);
    }
  }
}
