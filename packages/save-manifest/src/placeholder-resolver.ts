export interface PlaceholderContext {
  home: string;
  winLocalAppData: string;
  winAppData: string;
  winDocuments: string;
  storeGameDir: string;
  storeUserDir: string;
  storeUserId: string;
  root: string;
  steamId?: string;
}

const MAP: Record<string, keyof PlaceholderContext> = {
  '<home>': 'home',
  '<winLocalAppData>': 'winLocalAppData',
  '<winAppData>': 'winAppData',
  '<winDocuments>': 'winDocuments',
  '<storeGameDir>': 'storeGameDir',
  '<storeUserDir>': 'storeUserDir',
  '<storeUserId>': 'storeUserId',
  '<root>': 'root',
};

export function resolvePlaceholders(path: string, ctx: PlaceholderContext): string {
  let out = path.replace(/\\/g, '/');
  for (const [token, key] of Object.entries(MAP)) {
    const value = ctx[key];
    if (typeof value === 'string') {
      out = out.split(token).join(value.replace(/\\/g, '/'));
    }
  }
  if (ctx.steamId) {
    out = out.split('<steamId>').join(ctx.steamId);
  }
  return out.replace(/\//g, '\\');
}
