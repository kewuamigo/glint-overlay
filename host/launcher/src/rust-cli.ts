import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { projectRoot } from './paths.js';

export function resolveLauncherBin(): string {
  if (process.env.GLINT_LAUNCHER_BIN) {
    return process.env.GLINT_LAUNCHER_BIN;
  }
  const root = projectRoot();
  const release = path.join(root, 'target/release/glint-launcher.exe');
  if (fs.existsSync(release)) return release;
  const debug = path.join(root, 'target/debug/glint-launcher.exe');
  if (fs.existsSync(debug)) return debug;
  throw new Error(
    'glint-launcher.exe not found — build with: cargo build -p glint-launcher',
  );
}

export function rustCli<T>(command: string, extraArgs: string[] = []): T {
  const bin = resolveLauncherBin();
  const result = spawnSync(bin, ['--cli', command, ...extraArgs], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const msg = result.stderr?.trim() || result.stdout?.trim() || `exit ${result.status}`;
    throw new Error(`launcher --cli ${command}: ${msg}`);
  }
  const line = result.stdout.trim().split('\n').pop() ?? '{}';
  return JSON.parse(line) as T;
}

/** Non-blocking — use for inject (UAC prompt can take several seconds). */
export function rustCliAsync<T>(
  command: string,
  extraArgs: string[] = [],
): Promise<T> {
  return new Promise((resolve, reject) => {
    const bin = resolveLauncherBin();
    const child = spawn(bin, ['--cli', command, ...extraArgs], {
      windowsHide: true,
    });
    let stdout = '';
    let stderr = '';
    child.stdout?.on('data', (chunk: Buffer | string) => {
      stdout += chunk;
    });
    child.stderr?.on('data', (chunk: Buffer | string) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code !== 0) {
        reject(
          new Error(
            `launcher --cli ${command}: ${stderr.trim() || stdout.trim() || `exit ${code}`}`,
          ),
        );
        return;
      }
      const line = stdout.trim().split('\n').pop() ?? '{}';
      resolve(JSON.parse(line) as T);
    });
  });
}
