import { createServer, type Server } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';

const MIME: Record<string, string> = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
};

/** Serve a built UI bundle from disk (Vite dist). */
export async function startStaticServer(
  distDir: string,
  port: number,
): Promise<Server> {
  const root = path.resolve(distDir);
  const server = createServer(async (req, res) => {
    try {
      const urlPath = req.url?.split('?')[0] ?? '/';
      const rel = urlPath === '/' ? '/index.html' : urlPath;
      const filePath = path.join(root, rel);
      const info = await stat(filePath);
      if (!info.isFile()) {
        res.writeHead(404);
        res.end('Not found');
        return;
      }
      const ext = path.extname(filePath);
      const body = await readFile(filePath);
      res.writeHead(200, {
        'Content-Type': MIME[ext] ?? 'application/octet-stream',
      });
      res.end(body);
    } catch {
      res.writeHead(404);
      res.end('Not found');
    }
  });

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => resolve());
  });
  return server;
}

export function staticServerUrl(port: number): string {
  return `http://127.0.0.1:${port}`;
}
