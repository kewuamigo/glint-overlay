import fs from 'node:fs/promises';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { c as tarCreate, x as tarExtract } from 'tar';

/** Pack a directory into an uncompressed tar archive (pax for long paths). */
export async function packTar(dir: string): Promise<Buffer> {
  const chunks: Buffer[] = [];
  const stream = tarCreate({ cwd: dir }, ['.']);
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks);
}

/** Extract an uncompressed tar archive into dest (path traversal blocked by tar). */
export async function unpackTar(archive: Buffer, dest: string): Promise<void> {
  await fs.mkdir(dest, { recursive: true });
  await pipeline(Readable.from(archive), tarExtract({ cwd: dest }));
}
