#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { startCloudSyncServer } from './server.js';

function env(name: string, fallback?: string): string {
  const v = process.env[name] ?? fallback;
  if (v === undefined || v === '') {
    console.error(`Missing required env: ${name}`);
    process.exit(1);
  }
  return v;
}

const token = env('CLOUD_SYNC_TOKEN');
const root = path.resolve(env('CLOUD_SYNC_ROOT', './cloud-sync-data'));
const port = Number(env('CLOUD_SYNC_PORT', '8787'));
const host = process.env.CLOUD_SYNC_HOST ?? '127.0.0.1';

await fs.mkdir(root, { recursive: true });

const server = await startCloudSyncServer({ root, token, host, port });
console.log(
  `cloud-sync-server listening on http://${host}:${port}  root=${root}`,
);

const shutdown = () => {
  server.close(() => process.exit(0));
};
process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);
