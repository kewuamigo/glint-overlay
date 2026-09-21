import { useCallback, useEffect, useState } from 'react';
import { motion } from 'framer-motion';
import {
  launcherInvoke,
  type GameArt,
  type PrepareState,
  type ScannedGame,
} from '../launcher-bridge';
import { gameAccent } from '../lib/gameVisual';
import { ProgressRing } from './ProgressRing';
import { PrepareFailedBanner, PrepareProgress } from './PrepareFailedBanner';

type AchievementRow = {
  achievement_id: string;
  title: string;
  description: string | null;
  icon_unlocked: string | null;
  unlocked: number;
};

type EmuGuide = {
  title: string;
  downloadUrl: string;
  steps: string[];
};

type Props = {
  games: ScannedGame[];
  covers: Record<string, GameArt>;
  prepareStatuses?: Record<string, PrepareState>;
  forcingGse?: Record<string, true>;
  onRetryPrepare?: (
    game: ScannedGame,
    steamAppId?: string,
    forceGse?: boolean,
  ) => void;
};

export function AchievementsView({
  games,
  covers,
  prepareStatuses,
  forcingGse,
  onRetryPrepare,
}: Props) {
  const [selected, setSelected] = useState<string | null>(null);
  const [rows, setRows] = useState<AchievementRow[]>([]);
  const [guide, setGuide] = useState<EmuGuide | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [guideOpen, setGuideOpen] = useState(false);

  const sync = useCallback(async () => {
    if (games.length === 0) return;
    const res = await launcherInvoke<{
      games: unknown[];
      guide: EmuGuide;
    }>('achievements.syncFromLibrary', [
      games.map((g) => ({
        id: g.id,
        name: g.name,
        exe: g.exe,
        install_path: g.install_path,
      })),
    ]);
    if (res?.guide) setGuide(res.guide);
    if (selected) {
      const list = await launcherInvoke<AchievementRow[]>(
        'achievements.listForGame',
        [selected],
      );
      setRows(Array.isArray(list) ? list : []);
    }
  }, [games, selected]);

  useEffect(() => {
    void sync().catch((err) =>
      setStatus(err instanceof Error ? err.message : String(err)),
    );
  }, [sync]);

  useEffect(() => {
    if (!selected && games[0]) setSelected(games[0].id);
  }, [games, selected]);

  useEffect(() => {
    if (!selected) {
      setRows([]);
      return;
    }
    void launcherInvoke<AchievementRow[]>('achievements.listForGame', [selected]).then(
      (list) => setRows(Array.isArray(list) ? list : []),
    );
  }, [selected, games]);

  const unlocked = rows.filter((r) => r.unlocked).length;
  const selectedGame = games.find((g) => g.id === selected) ?? null;
  const selectedPrepare = selectedGame
    ? prepareStatuses?.[selectedGame.id]
    : undefined;
  const selectedPreparing = selectedPrepare?.status === 'pending';
  const selectedFailed = selectedPrepare?.status === 'failed';
  const pct = rows.length > 0 ? Math.round((unlocked / rows.length) * 100) : 0;
  const hero = selectedGame ? covers[selectedGame.id]?.hero : undefined;
  const accent = selectedGame ? gameAccent(selectedGame.name) : null;

  return (
    <div className="achievements-page">
      <div className="library-header">
        <div>
          <p className="page-eyebrow">Trophy room</p>
          <h1 className="page-title">Achievements</h1>
          <p className="home-muted">
            Paths come from Library. Pick a title to browse unlocks.
          </p>
        </div>
      </div>

      {status && <p className="home-muted">{status}</p>}

      <div className="achievements-layout">
        <aside className="achievements-games glass">
          {games.length === 0 ? (
            <p className="home-muted">Library is empty — add a game exe in Library.</p>
          ) : (
            games.map((g) => {
              const icon = covers[g.id]?.icon;
              const { hue, initials } = gameAccent(g.name);
              return (
                <button
                  key={g.id}
                  type="button"
                  className={`achievements-game${selected === g.id ? ' active' : ''}`}
                  onClick={() => setSelected(g.id)}
                >
                  <span
                    className="achievements-game-thumb"
                    style={
                      icon
                        ? undefined
                        : {
                            background: `linear-gradient(160deg, hsl(${hue} 36% 28%), hsl(${hue} 22% 12%))`,
                          }
                    }
                  >
                    {icon ? <img src={icon} alt="" /> : initials}
                  </span>
                  <span className="achievements-game-copy">
                    <strong>{g.name}</strong>
                    <span>{g.exe}</span>
                  </span>
                </button>
              );
            })
          )}
        </aside>

        <section className="achievements-detail">
          {!selectedGame || !accent ? (
            <div className="empty-state glass">Select a library game.</div>
          ) : (
            <>
              <div
                className="achievements-showcase"
                style={
                  hero
                    ? undefined
                    : {
                        background: `linear-gradient(120deg, hsl(${accent.hue} 42% 20%) 0%, #07080c 70%)`,
                      }
                }
              >
                {hero && (
                  <img className="achievements-showcase-cover" src={hero} alt="" />
                )}
                <div className="achievements-showcase-shade" />
                <div className="achievements-showcase-body">
                  <ProgressRing pct={pct} size={88} />
                  <div>
                    <div className="achievements-showcase-actions">
                      <span className="pill">{selectedGame.source}</span>
                      {onRetryPrepare && (
                        <button
                          type="button"
                          className="btn-secondary"
                          disabled={selectedPreparing}
                          onClick={() =>
                            onRetryPrepare(selectedGame, undefined, true)
                          }
                        >
                          {selectedPreparing ? 'Installing…' : 'Install GSE'}
                        </button>
                      )}
                    </div>
                    <h2>{selectedGame.name}</h2>
                    <p>
                      {rows.length > 0
                        ? `${unlocked} / ${rows.length} unlocked`
                        : 'No achievement schema found yet'}
                    </p>
                    <div className="progress-bar">
                      <motion.div
                        className="progress-fill"
                        initial={{ width: 0 }}
                        animate={{ width: `${pct}%` }}
                        transition={{ duration: 0.55, ease: 'easeOut' }}
                      />
                    </div>
                    {selectedPreparing && (
                      <PrepareProgress
                        forceGse={Boolean(forcingGse?.[selectedGame.id])}
                      />
                    )}
                    {selectedFailed && onRetryPrepare && (
                      <PrepareFailedBanner
                        prepare={selectedPrepare}
                        game={selectedGame}
                        errorClassName="cloud-sync-status-line"
                        onRetry={onRetryPrepare}
                      />
                    )}
                  </div>
                </div>
              </div>

              <div className="trophy-grid">
                {rows.map((r, i) => (
                  <motion.article
                    key={r.achievement_id}
                    className={`trophy-card glass${r.unlocked ? ' on' : ' locked'}`}
                    initial={{ opacity: 0, y: 10 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ delay: Math.min(i, 12) * 0.03, duration: 0.22 }}
                  >
                    {r.icon_unlocked ? (
                      <img src={r.icon_unlocked} alt="" />
                    ) : (
                      <span className="achievements-dot" />
                    )}
                    <div>
                      <strong>{r.title}</strong>
                      {r.description && <p>{r.description}</p>}
                      <em>{r.unlocked ? 'Unlocked' : 'Locked'}</em>
                    </div>
                  </motion.article>
                ))}
                {rows.length === 0 && (
                  <p className="home-muted">
                    Run fetch_epic_achievements.bat / generate GSE schema next to
                    the exe, then reopen this tab.
                  </p>
                )}
              </div>
            </>
          )}
        </section>
      </div>

      {guide && (
        <div className="achievements-guide glass">
          <button
            type="button"
            className="achievements-guide-toggle"
            onClick={() => setGuideOpen((v) => !v)}
            aria-expanded={guideOpen}
          >
            <h3>{guide.title}</h3>
            <span>{guideOpen ? 'Hide' : 'How to enable'}</span>
          </button>
          {guideOpen && (
            <>
              <ol>
                {guide.steps.map((step) => (
                  <li key={step}>{step}</li>
                ))}
              </ol>
              <button
                type="button"
                className="btn-secondary"
                onClick={() =>
                  void launcherInvoke('achievements.openUrl', [guide.downloadUrl])
                }
              >
                Open download / docs
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
