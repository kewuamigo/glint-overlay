import { motion } from 'framer-motion';
import type { InstalledApp, StoreCatalog } from '../launcher-bridge';

type Props = {
  catalog: StoreCatalog | null;
  installed: InstalledApp[];
  installing: string | null;
  onInstall: (id: string) => void;
};

export function StoreView({ catalog, installed, installing, onInstall }: Props) {
  const installedIds = new Set(installed.map((a) => a.id));
  const apps = catalog?.apps ?? [];

  return (
    <div>
      <p className="page-eyebrow">Extensions</p>
      <h1 className="page-title">Store</h1>
      {apps.length === 0 ? (
        <div className="empty-state glass">No apps in catalog.</div>
      ) : (
        <div className="store-grid">
          {apps.map((app, i) => {
            const isInstalled = installedIds.has(app.id);
            return (
              <motion.article
                key={app.id}
                className="store-card glass"
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: i * 0.04, duration: 0.25 }}
              >
                <h3>{app.name}</h3>
                <p className="store-desc">{app.description}</p>
                <span className="store-version">v{app.version}</span>
                {isInstalled ? (
                  <span className="installed-tag">Installed</span>
                ) : (
                  <button
                    type="button"
                    className="btn-primary"
                    disabled={installing === app.id}
                    onClick={() => onInstall(app.id)}
                  >
                    {installing === app.id ? 'Installing…' : 'Install'}
                  </button>
                )}
              </motion.article>
            );
          })}
        </div>
      )}
    </div>
  );
}
