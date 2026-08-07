import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  startStaticServer,
  staticServerUrl,
} from '@glint/static-server';

function resolveUiDist(): string {
  if (process.env.GLINT_LAUNCHER_UI_DIST) {
    return process.env.GLINT_LAUNCHER_UI_DIST;
  }
  const here = path.dirname(fileURLToPath(import.meta.url));
  for (const rel of ['../ui', '../../ui']) {
    const candidate = path.resolve(here, rel);
    if (fs.existsSync(path.join(candidate, 'index.html'))) {
      return candidate;
    }
  }
  return path.resolve(here, '../../../ui/launcher/dist');
}

let boundPort = 5174;

export async function startLauncherUiServer(preferredPort = 5174): Promise<number> {
  const uiDist = resolveUiDist();
  for (let port = preferredPort; port < preferredPort + 20; port++) {
    try {
      await startStaticServer(uiDist, port);
      boundPort = port;
      return port;
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code === 'EADDRINUSE') {
        continue;
      }
      throw err;
    }
  }
  throw new Error(`no free port for launcher UI (${preferredPort}–${preferredPort + 19})`);
}

export function getLauncherUiUrl(): string {
  if (process.env.GLINT_LAUNCHER_UI_URL) {
    return process.env.GLINT_LAUNCHER_UI_URL;
  }
  const envPort = Number(process.env.GLINT_LAUNCHER_UI_PORT);
  const port = Number.isFinite(envPort) && envPort > 0 ? envPort : boundPort;
  return staticServerUrl(port);
}
