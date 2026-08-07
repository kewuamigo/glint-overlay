import fs from 'node:fs';
import path from 'node:path';
import { z } from 'zod';
import { APP_DATA_DIR_NAME } from './product.js';

const manifestSchema = z.object({
  id: z.string().regex(/^[a-z0-9-]+$/),
  name: z.string().min(1),
  version: z.string().min(1),
  entry: z.string().min(1),
  icon: z.string().min(1).optional(),
});

export type InstalledAppSummary = {
  id: string;
  name: string;
  version: string;
  icon?: string;
};

export function appsRoot(): string {
  if (process.env.GLINT_APPS_DIR) {
    return process.env.GLINT_APPS_DIR;
  }
  return path.join(process.env.APPDATA ?? '', APP_DATA_DIR_NAME, 'apps');
}

export function listInstalledApps(): InstalledAppSummary[] {
  const root = appsRoot();
  if (!fs.existsSync(root)) return [];

  const out: InstalledAppSummary[] = [];
  for (const id of fs.readdirSync(root)) {
    const dir = path.join(root, id);
    const manifestPath = path.join(dir, 'manifest.json');
    if (!fs.existsSync(manifestPath)) continue;
    try {
      const raw = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
      const parsed = manifestSchema.parse(raw);
      if (parsed.id !== id) continue;
      const entryPath = path.join(dir, parsed.entry.replace(/^\.\//, ''));
      if (!fs.existsSync(entryPath)) continue;
      out.push({
        id: parsed.id,
        name: parsed.name,
        version: parsed.version,
        icon: parsed.icon,
      });
    } catch {
      // skip invalid app folder
    }
  }
  out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

export function isAppInstalled(id: string): boolean {
  return listInstalledApps().some((app) => app.id === id);
}
