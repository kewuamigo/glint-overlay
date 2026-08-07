import net from 'node:net';

type CmServer = {
  endpoint: string;
  dc?: string;
  wtd_load?: number;
};

type PingTarget = {
  host: string;
  port: number;
  dc?: string;
};

/** Official Valve CM directory — sorted for the caller's public IP (no key required). */
const CM_LIST_URL =
  'https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/?cmtype=netfilter&maxcount=5';

const LIST_CACHE_MS = 60_000;

let listCache: { at: number; servers: CmServer[] } | null = null;

async function fetchNearestCmServers(): Promise<CmServer[]> {
  if (listCache && Date.now() - listCache.at < LIST_CACHE_MS) {
    return listCache.servers;
  }
  const res = await fetch(CM_LIST_URL);
  if (!res.ok) throw new Error(`CM list HTTP ${res.status}`);
  const json = (await res.json()) as {
    response?: { serverlist?: CmServer[]; success?: boolean };
  };
  const servers = json.response?.serverlist ?? [];
  if (servers.length === 0) throw new Error('empty CM server list');
  listCache = { at: Date.now(), servers };
  return servers;
}

function parseEndpoint(endpoint: string): PingTarget {
  const idx = endpoint.lastIndexOf(':');
  if (idx <= 0) throw new Error(`bad endpoint: ${endpoint}`);
  return {
    host: endpoint.slice(0, idx),
    port: Number(endpoint.slice(idx + 1)) || 27017,
  };
}

function tcpPingMs(host: string, port: number): Promise<number> {
  return new Promise((resolve, reject) => {
    const start = Date.now();
    const socket = net.connect({ host, port }, () => {
      const ms = Date.now() - start;
      socket.destroy();
      resolve(ms);
    });
    socket.setTimeout(3000);
    socket.on('timeout', () => {
      socket.destroy();
      reject(new Error('ping timeout'));
    });
    socket.on('error', (err) => {
      socket.destroy();
      reject(err);
    });
  });
}

/** Refresh CM list (cached ~60s), ping first entry (Valve-sorted nearest). */
export async function steamNearestPing(): Promise<{
  ms: number;
  host: string;
  port: number;
  dc?: string;
}> {
  const servers = await fetchNearestCmServers();
  let lastErr: unknown;
  for (const server of servers) {
    try {
      const { host, port } = parseEndpoint(server.endpoint);
      const ms = await tcpPingMs(host, port);
      return { ms, host, port, dc: server.dc };
    } catch (err) {
      lastErr = err;
    }
  }
  throw lastErr instanceof Error ? lastErr : new Error('all CM pings failed');
}
