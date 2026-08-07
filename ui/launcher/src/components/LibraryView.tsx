import { motion } from 'framer-motion';
import type { GameArt, ScannedGame } from '../launcher-bridge';
import { gameAccent, formatPlaytime } from '../lib/gameVisual';
import { useCloudSyncPlayState } from '../hooks/useCloudSyncPlayState';
import { AddGameForm } from './FriendList';
import { CloudSyncGameActions } from './CloudSyncGameActions';

type Props = {
  games: ScannedGame[];
  featured: ScannedGame | null;
  launching: string | null;
  covers: Record<string, GameArt>;
  onAttach: (game: ScannedGame) => void;
  onPlay: (game: ScannedGame) => void;
  onSelectGame: (game: ScannedGame) => void;
  onAddGame: (name: string, exe: string) => void;
  onReset: () => void;
};

const grid = {
  hidden: {},
  show: { transition: { staggerChildren: 0.035 } },
};

const tile = {
  hidden: { opacity: 0, y: 10 },
  show: { opacity: 1, y: 0, transition: { duration: 0.22 } },
};

export function LibraryView({
  games,
  featured,
  launching,
  covers,
  onAttach,
  onPlay,
  onSelectGame,
  onAddGame,
  onReset,
}: Props) {
  const others = games.filter((g) => g.id !== featured?.id);
  const feat = featured ? gameAccent(featured.name) : null;
  const featuredHero = featured ? covers[featured.id]?.hero : undefined;
  const featuredPlay = featured ? formatPlaytime(featured.playtime_hours) : null;
  const { syncing } = useCloudSyncPlayState(featured?.id);
  const playBusy = featured
    ? launching === featured.id || syncing
    : false;
  const playLabel = syncing
    ? 'Syncing…'
    : featured && launching === featured.id
      ? 'Starting…'
      : 'Play';

  return (
    <div>
      <div className="library-header">
        <div>
          <p className="page-eyebrow">Collection</p>
          <h1 className="page-title">Library</h1>
        </div>
        <div className="library-actions">
          <button type="button" className="btn-secondary" onClick={onReset}>
            Rescan library
          </button>
          <AddGameForm onAdd={onAddGame} />
        </div>
      </div>

      {featured && feat ? (
        <motion.div
          className="featured-card"
          initial={{ opacity: 0, y: 12 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }}
          style={{
            background: featuredHero
              ? undefined
              : `linear-gradient(145deg, hsl(${feat.hue} 42% 22%) 0%, hsl(${feat.hue} 28% 10%) 55%, #0c0e14 100%)`,
          }}
        >
          {featuredHero && (
            <img className="featured-cover" src={featuredHero} alt="" />
          )}
          <div className="featured-body">
            <div className="featured-top">
              <span className="pill">{featured.source}</span>
              {featured.running && <span className="pill pill-live">Running</span>}
              {syncing && <span className="pill pill-sync">Cloud</span>}
            </div>
            <h2 className="featured-name">{featured.name}</h2>
            <p className="featured-meta">
              {featured.exe}
              {featured.running && featured.pid ? ` · PID ${featured.pid}` : ''}
              {featuredPlay ? ` · ${featuredPlay} played` : ''}
            </p>
            {featured.running && featured.pid ? (
              <motion.button
                type="button"
                className="btn-primary"
                disabled={
                  launching === String(featured.pid) || launching === featured.id
                }
                onClick={() => onAttach(featured)}
                whileHover={{ scale: 1.02 }}
                whileTap={{ scale: 0.96 }}
              >
                {launching === String(featured.pid)
                  ? 'Attaching overlay…'
                  : 'Attach overlay'}
              </motion.button>
            ) : (
              <motion.button
                type="button"
                className={`btn-primary${syncing ? ' btn-primary--syncing' : ''}`}
                disabled={playBusy}
                onClick={() => onPlay(featured)}
                whileHover={playBusy ? undefined : { scale: 1.02 }}
                whileTap={playBusy ? undefined : { scale: 0.96 }}
                aria-busy={syncing || undefined}
              >
                <span className="btn-play-label">{playLabel}</span>
              </motion.button>
            )}
            <CloudSyncGameActions game={featured} />
          </div>
          {!featuredHero && (
            <div className="featured-watermark">{feat.initials}</div>
          )}
        </motion.div>
      ) : (
        <div className="empty-state glass">
          No games yet — add one or install from Steam.
        </div>
      )}

      {others.length > 0 && (
        <motion.div
          className="game-grid"
          variants={grid}
          initial="hidden"
          animate="show"
        >
          {others.map((game) => {
            const { hue, initials } = gameAccent(game.name);
            const gridArt = covers[game.id]?.grid;
            const play = formatPlaytime(game.playtime_hours);
            return (
              <motion.button
                key={game.id}
                type="button"
                variants={tile}
                className={`game-tile${game.running ? ' running' : ''}`}
                onClick={() => onSelectGame(game)}
                whileHover={{
                  y: -5,
                  scale: 1.02,
                  transition: { duration: 0.15 },
                }}
                whileTap={{ scale: 0.97 }}
                style={{
                  background: gridArt
                    ? undefined
                    : `linear-gradient(160deg, hsl(${hue} 38% 28%) 0%, hsl(${hue} 24% 14%) 100%)`,
                }}
              >
                {gridArt ? (
                  <img className="game-tile-cover" src={gridArt} alt="" />
                ) : (
                  <span className="game-tile-art">{initials}</span>
                )}
                {game.running && <span className="game-tile-badge">Live</span>}
                <div className="game-tile-label">
                  <span>{game.name}</span>
                  {play && <span className="game-tile-playtime">{play}</span>}
                </div>
              </motion.button>
            );
          })}
        </motion.div>
      )}
    </div>
  );
}
