import { useEffect, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import {
  launcherInvoke,
  type ArtSlot,
  type GameArt,
  type ScannedGame,
  type SgdbAsset,
} from '../launcher-bridge';
import { ArtMedia } from './ArtMedia';

const TABS: { id: ArtSlot; label: string }[] = [
  { id: 'logo', label: 'Logo' },
  { id: 'hero', label: 'Hero' },
  { id: 'grid', label: 'Grid' },
  { id: 'icon', label: 'Icon' },
];

type Props = {
  game: ScannedGame;
  initialSlot?: ArtSlot;
  onClose: () => void;
  onApplied: (art: GameArt) => void;
};

export function ArtPickerModal({
  game,
  initialSlot = 'logo',
  onClose,
  onApplied,
}: Props) {
  const [slot, setSlot] = useState<ArtSlot>(initialSlot);
  const [assets, setAssets] = useState<SgdbAsset[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setAssets([]);
    void launcherInvoke<SgdbAsset[]>('covers.listAssets', [
      game.id,
      game.name,
      slot,
    ])
      .then((list) => {
        if (!cancelled) {
          setAssets(Array.isArray(list) ? list : []);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [game.id, game.name, slot]);

  const apply = async (asset: SgdbAsset) => {
    setBusy(true);
    setError(null);
    try {
      const art = await launcherInvoke<GameArt>('covers.setAsset', [
        game.id,
        slot,
        asset.url,
        asset.mime,
      ]);
      onApplied(art ?? {});
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    setBusy(true);
    setError(null);
    try {
      const art = await launcherInvoke<GameArt>('covers.clearAsset', [
        game.id,
        game.name,
        slot,
      ]);
      onApplied(art ?? {});
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <motion.div
      className="art-picker-backdrop"
      role="presentation"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.16, ease: 'easeOut' }}
      onClick={onClose}
    >
      <motion.div
        className="art-picker glass"
        role="dialog"
        aria-modal="true"
        aria-label={`Customize artwork for ${game.name}`}
        initial={{ opacity: 0, y: 10, scale: 0.98 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: 6, scale: 0.98 }}
        transition={{ duration: 0.2, ease: [0.22, 1, 0.36, 1] }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="art-picker-head">
          <div>
            <p className="art-picker-eyebrow">SteamGridDB</p>
            <h2 className="art-picker-title">{game.name}</h2>
          </div>
          <button
            type="button"
            className="btn-icon-gear"
            aria-label="Close"
            onClick={onClose}
          >
            ✕
          </button>
        </div>

        <div className="art-picker-tabs" role="tablist">
          {TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={slot === tab.id}
              className={`art-picker-tab${slot === tab.id ? ' active' : ''}`}
              onClick={() => setSlot(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>

        {error && <p className="art-picker-error">{error}</p>}

        <div className="art-picker-body">
          {loading ? (
            <p className="home-muted">Loading artwork…</p>
          ) : assets.length === 0 ? (
            <p className="home-muted">No assets for this slot.</p>
          ) : (
            <div className="art-picker-grid">
              <AnimatePresence initial={false}>
                {assets.map((asset, i) => (
                  <motion.button
                    key={asset.id}
                    type="button"
                    className={`art-picker-thumb art-picker-thumb--${slot}`}
                    disabled={busy}
                    onClick={() => void apply(asset)}
                    initial={{ opacity: 0, y: 6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{
                      duration: 0.18,
                      delay: Math.min(i, 12) * 0.04,
                      ease: 'easeOut',
                    }}
                    whileTap={busy ? undefined : { scale: 0.96 }}
                  >
                    <ArtMedia
                      className="art-picker-thumb-media"
                      src={asset.thumb || asset.url}
                      mime={asset.mime}
                      alt=""
                    />
                    {asset.animated && (
                      <span className="art-picker-badge">Animated</span>
                    )}
                  </motion.button>
                ))}
              </AnimatePresence>
            </div>
          )}
        </div>

        <div className="art-picker-foot">
          <button
            type="button"
            className="btn-ghost"
            disabled={busy}
            onClick={() => void reset()}
          >
            Reset to automatic
          </button>
          <button type="button" className="btn-secondary" onClick={onClose}>
            Done
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}

/** Compact gear control that matches ghost buttons. */
export function ArtGearButton({
  onClick,
  className = '',
  label = 'Customize artwork',
}: {
  onClick: () => void;
  className?: string;
  label?: string;
}) {
  return (
    <motion.button
      type="button"
      className={`btn-icon-gear${className ? ` ${className}` : ''}`}
      aria-label={label}
      title={label}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      whileHover={{ scale: 1.04 }}
      whileTap={{ scale: 0.96 }}
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    </motion.button>
  );
}
