import fs from 'node:fs';
import path from 'node:path';
import { z } from 'zod';
import { projectRoot } from './paths.js';
import { appsRoot } from './apps-scan.js';

const catalogSchema = z.object({
  version: z.number(),
  apps: z.array(
    z.object({
      id: z.string().regex(/^[a-z0-9-]+$/),
      name: z.string().min(1),
      version: z.string().min(1),
      description: z.string(),
      iconUrl: z.string().optional(),
      downloadUrl: z.string().min(1),
    }),
  ),
});

export type StoreCatalog = z.infer<typeof catalogSchema>;
export type StoreApp = StoreCatalog['apps'][number];

let cachedCatalog: StoreCatalog | null = null;

export function defaultCatalogPath(): string {
  return path.join(projectRoot(), 'config/store-catalog.json');
}

export async function loadCatalog(): Promise<StoreCatalog> {
  const url = process.env.GLINT_STORE_CATALOG;
  if (url) {
    if (url.startsWith('http://') || url.startsWith('https://')) {
      const res = await fetch(url);
      if (!res.ok) {
        throw new Error(`catalog fetch failed: ${res.status}`);
      }
      const json = await res.json();
      cachedCatalog = catalogSchema.parse(json);
      return cachedCatalog;
    }
    const filePath = path.isAbsolute(url) ? url : path.join(projectRoot(), url);
    const json = JSON.parse(fs.readFileSync(filePath, 'utf8'));
    cachedCatalog = catalogSchema.parse(json);
    return cachedCatalog;
  }

  const local = defaultCatalogPath();
  const json = JSON.parse(fs.readFileSync(local, 'utf8'));
  cachedCatalog = catalogSchema.parse(json);
  return cachedCatalog;
}

export function findCatalogApp(id: string): StoreApp | undefined {
  return cachedCatalog?.apps.find((app) => app.id === id);
}
