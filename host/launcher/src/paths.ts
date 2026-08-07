import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

/** Locate install root (packaged) or monorepo root (dev). */
export function projectRoot(): string {
  if (process.env.GLINT_PROJECT_ROOT) {
    return process.env.GLINT_PROJECT_ROOT;
  }
  const here = path.dirname(fileURLToPath(import.meta.url));
  for (const rel of ['../..', '../../..'] as const) {
    const candidate = path.resolve(here, rel);
    if (
      fs.existsSync(path.join(candidate, 'host')) &&
      fs.existsSync(path.join(candidate, 'native'))
    ) {
      return candidate;
    }
  }
  return path.resolve(here, '../../..');
}
