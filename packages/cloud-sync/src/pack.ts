import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import type {
  PackRevisionInput,
  PackRevisionResult,
  SyncRevisionManifest,
  SyncSaveRootEntry,
  UnpackRevisionResult,
} from './types.js';

const MANIFEST_FILE = 'manifest.json';
const SAVES_DIR = 'saves';
const ACHIEVEMENTS_FILE = 'achievements.json';

async function exists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

async function hashBytes(data: Buffer | string): Promise<string> {
  return createHash('sha256').update(data).digest('hex');
}

/** Stable content hash of a file tree (relative paths + file bytes). */
export async function hashTree(root: string): Promise<string | null> {
  if (!(await exists(root))) return null;

  const hash = createHash('sha256');
  let fileCount = 0;

  async function walk(dir: string, rel: string): Promise<void> {
    let entries;
    try {
      entries = await fs.readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      const childRel = rel ? `${rel}/${entry.name}` : entry.name;
      const childPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await walk(childPath, childRel);
      } else if (entry.isFile()) {
        fileCount += 1;
        const data = await fs.readFile(childPath);
        hash.update(childRel);
        hash.update('\0');
        hash.update(data);
      }
    }
  }

  await walk(root, '');
  if (fileCount === 0) return null;
  return hash.digest('hex');
}

function uniqueRootName(sourcePath: string, used: Set<string>): string {
  let base = path.basename(sourcePath) || 'root';
  base = base.replace(/[<>:"|?*]/g, '_');
  let name = base;
  let n = 2;
  while (used.has(name)) {
    name = `${base}-${n++}`;
  }
  used.add(name);
  return name;
}

function makeRevisionId(createdAt: Date): string {
  return createdAt.toISOString().replace(/[:.]/g, '-');
}

/**
 * Pack a revision directory: `manifest.json` + `saves/` + optional `achievements.json`.
 * Empty/missing save roots and omitted achievements do not fail the pack.
 */
export async function packRevision(
  destDir: string,
  input: PackRevisionInput,
): Promise<PackRevisionResult> {
  const createdAt = input.createdAt ?? new Date();
  const revisionId = input.revisionId ?? makeRevisionId(createdAt);
  const revisionDir = path.join(destDir, revisionId);
  const savesDir = path.join(revisionDir, SAVES_DIR);

  await fs.mkdir(savesDir, { recursive: true });

  const usedNames = new Set<string>();
  const roots: SyncSaveRootEntry[] = [];

  for (const sourcePath of input.saveLocations ?? []) {
    const name = uniqueRootName(sourcePath, usedNames);
    const target = path.join(savesDir, name);

    let contentHash: string | null = null;
    let st: Awaited<ReturnType<typeof fs.stat>> | null = null;
    try {
      st = await fs.stat(sourcePath);
    } catch {
      st = null;
    }

    if (!st) {
      await fs.mkdir(target, { recursive: true });
    } else if (st.isFile()) {
      await fs.cp(sourcePath, target);
      contentHash = await hashBytes(await fs.readFile(target));
    } else {
      await fs.cp(sourcePath, target, { recursive: true });
      contentHash = await hashTree(target);
    }

    roots.push({ name, sourcePath, contentHash });
  }

  let achievementsPath: 'achievements.json' | null = null;
  let achievementsHash: string | null = null;

  if (input.achievements !== undefined) {
    const json = JSON.stringify(input.achievements, null, 2);
    await fs.writeFile(path.join(revisionDir, ACHIEVEMENTS_FILE), json, 'utf8');
    achievementsPath = ACHIEVEMENTS_FILE;
    achievementsHash = await hashBytes(json);
  }

  const savesHash = await hashTree(savesDir);
  const contentHash = await hashBytes(
    `${savesHash ?? ''}\n${achievementsHash ?? ''}`,
  );

  const manifest: SyncRevisionManifest = {
    version: 1,
    gameId: input.gameId,
    revisionId,
    createdAt: createdAt.toISOString(),
    contentHash,
    saves: {
      path: SAVES_DIR,
      roots,
      contentHash: savesHash,
    },
    achievements: {
      path: achievementsPath,
      contentHash: achievementsHash,
    },
  };

  await fs.writeFile(
    path.join(revisionDir, MANIFEST_FILE),
    JSON.stringify(manifest, null, 2),
    'utf8',
  );

  return { revisionDir, manifest };
}

export async function readRevisionManifest(
  revisionDir: string,
): Promise<SyncRevisionManifest> {
  const raw = await fs.readFile(path.join(revisionDir, MANIFEST_FILE), 'utf8');
  return JSON.parse(raw) as SyncRevisionManifest;
}

export interface UnpackOptions {
  /**
   * Override restore destinations keyed by save root `name`.
   * Defaults to each root's `sourcePath` from the manifest.
   */
  saveTargets?: Record<string, string>;
  /** When false, skip writing save trees (achievements-only extract). Default true. */
  restoreSaves?: boolean;
}

/**
 * Unpack a revision: restore `saves/` trees and return achievements JSON.
 * Missing `achievements.json` or empty save trees are fine.
 */
export async function unpackRevision(
  revisionDir: string,
  options: UnpackOptions = {},
): Promise<UnpackRevisionResult> {
  const manifest = await readRevisionManifest(revisionDir);
  const restoreSaves = options.restoreSaves !== false;
  const restoredSaves: UnpackRevisionResult['restoredSaves'] = [];

  if (restoreSaves) {
    for (const root of manifest.saves.roots) {
      const src = path.join(revisionDir, SAVES_DIR, root.name);
      const dest = options.saveTargets?.[root.name] ?? root.sourcePath;
      if (!(await exists(src))) continue;

      const srcStat = await fs.stat(src);
      await fs.rm(dest, { recursive: true, force: true });
      await fs.mkdir(path.dirname(dest), { recursive: true });
      if (srcStat.isFile()) {
        await fs.cp(src, dest);
      } else {
        await fs.cp(src, dest, { recursive: true });
      }
      restoredSaves.push({ name: root.name, destPath: dest });
    }
  }

  let achievements: unknown | null = null;
  if (manifest.achievements.path) {
    const achPath = path.join(revisionDir, manifest.achievements.path);
    if (await exists(achPath)) {
      achievements = JSON.parse(await fs.readFile(achPath, 'utf8'));
    }
  }

  return { manifest, achievements, restoredSaves };
}
