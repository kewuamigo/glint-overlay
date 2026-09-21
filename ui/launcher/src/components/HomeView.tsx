import { useEffect, useState } from 'react';
import { motion } from 'framer-motion';
import {
  launcherInvoke,
  prepareBlocksLaunch,
  type GameArt,
  type PrepareState,
  type ScannedGame,
  type StoreCatalog,
} from '../launcher-bridge';
import { gameAccent, formatPlaytime, formatSource } from '../lib/gameVisual';
import { useCloudSyncPlayState } from '../hooks/useCloudSyncPlayState';
import {
  mockNews,
  mockSocialPosts,
  type MockFriend,
} from '../data/mockFriends';
import { FriendRail } from './FriendList';
import { ProgressRing } from './ProgressRing';
import { PrepareFailedBanner, PrepareProgress } from './PrepareFailedBanner';
import { ArtMedia } from './ArtMedia';
import { ArtGearButton } from './ArtPickerModal';

function HomePlayButton({
  gameId,
  launching,
  blocked,
  onPlay,
}: {
  gameId: string;
  launching: boolean;
  blocked: boolean;
  onPlay: () => void;
}) {
  const { syncing } = useCloudSyncPlayState(gameId);
  const busy = launching || syncing || blocked;
  const label = syncing ? 'Syncing…' : launching ? 'Starting…' : 'Play';
  return (
    <motion.button
      type="button"
      className={`btn-primary${syncing ? ' btn-primary--syncing' : ''}`}
      disabled={busy}
      onClick={onPlay}
      whileHover={busy ? undefined : { scale: 1.02 }}
      whileTap={busy ? undefined : { scale: 0.96 }}
      aria-busy={syncing || undefined}
    >
      {!syncing && (
        <span className="btn-play-icon" aria-hidden>
          ▶
        </span>
      )}
      <span className="btn-play-label">{label}</span>
    </motion.button>
  );
}

type AchievementRow = {
  achievement_id: string;
  title: string;
  description: string | null;
  icon_unlocked: string | null;
  unlocked: number;
};

type Props = {
  games: ScannedGame[];
  featured: ScannedGame | null;
  launching: string | null;
  prepareStatuses: Record<string, PrepareState>;
  covers: Record<string, GameArt>;
  catalog: StoreCatalog | null;
  isFavorite: (id: string) => boolean;
  onToggleFavorite: (id: string) => void;
  onAttach: (game: ScannedGame) => void;
  onPlay: (game: ScannedGame) => void;
  onRetryPrepare: (game: ScannedGame, steamAppId?: string, forceGse?: boolean) => void;
  onSelectGame: (game: ScannedGame) => void;
  onOpenLibrary: () => void;
  onOpenStore: () => void;
  onOpenAchievements: () => void;
  onSelectFriend: (friend: MockFriend) => void;
  onCustomizeArt?: (game: ScannedGame) => void;
  forcingGse?: boolean;
};

export function HomeView({
  games,
  featured,
  launching,
  prepareStatuses,
  covers,
  catalog,
  isFavorite,
  onToggleFavorite,
  onAttach,
  onPlay,
  onRetryPrepare,
  onSelectGame,
  onOpenLibrary,
  onOpenStore,
  onOpenAchievements,
  onSelectFriend,
  onCustomizeArt,
  forcingGse,
}: Props) {
  const feat = featured ? gameAccent(featured.name) : null;
  const featuredHero = featured ? covers[featured.id]?.hero : undefined;
  const featuredHeroMime = featured ? covers[featured.id]?.heroMime : undefined;
  const featuredLogo = featured ? covers[featured.id]?.logo : undefined;
  const featuredLogoMime = featured ? covers[featured.id]?.logoMime : undefined;
  const featuredPlay = featured ? formatPlaytime(featured.playtime_hours) : null;
  const featuredPrepare = featured
    ? prepareStatuses[featured.id]
    : undefined;
  const featuredCreating =
    featured != null &&
    (!featuredPrepare || featuredPrepare.status === 'pending');
  const featuredFailed = featuredPrepare?.status === 'failed';
  const railGames = games.slice(0, 6);
  const discoverGames = games.slice(0, 6);
  const catalogApps = catalog?.apps?.slice(0, 4) ?? [];
  const newsCards = [
    ...mockNews.map((n) => ({
      key: n.title,
      tag: n.tag,
      title: n.title,
      blurb: 'From the hub feed',
    })),
    ...catalogApps.slice(0, 2).map((app) => ({
      key: `app-${app.id}`,
      tag: 'Store',
      title: app.name,
      blurb: app.description,
    })),
  ].slice(0, 4);

  const [teaserRows, setTeaserRows] = useState<AchievementRow[]>([]);
  const [logoFailed, setLogoFailed] = useState(false);
  const teaserGame = featured ?? games[0] ?? null;

  useEffect(() => {
    setLogoFailed(false);
  }, [featuredLogo]);

  useEffect(() => {
    if (!teaserGame) {
      setTeaserRows([]);
      return;
    }
    let cancelled = false;
    void launcherInvoke<AchievementRow[]>('achievements.listForGame', [
      teaserGame.id,
    ])
      .then((list) => {
        if (!cancelled) setTeaserRows(Array.isArray(list) ? list : []);
      })
      .catch(() => {
        if (!cancelled) setTeaserRows([]);
      });
    return () => {
      cancelled = true;
    };
  }, [teaserGame?.id]);

  const unlocked = teaserRows.filter((r) => r.unlocked).length;
  const total = teaserRows.length;
  const pct = total > 0 ? Math.round((unlocked / total) * 100) : 0;
  const recentUnlocks = teaserRows.filter((r) => r.unlocked).slice(0, 4);

  return (
    <div className="home">
      <section className="home-stage">
        <div
          className="home-stage-bg"
          style={
            featuredHero
              ? undefined
              : featured && feat
                ? {
                    background: `linear-gradient(125deg, hsl(${feat.hue} 40% 22%) 0%, #07080c 62%)`,
                  }
                : undefined
          }
        >
          {featuredHero && (
            <ArtMedia
              className="home-stage-cover"
              src={featuredHero}
              mime={featuredHeroMime}
              alt=""
            />
          )}
          <div className="home-stage-shade" />
        </div>

        <div className="home-stage-body">
          <div className="home-hero-copy">
            {featured && feat ? (
              <>
                <span className={`pill${featured.running ? ' pill-live' : ''}`}>
                  {featured.running ? 'Now Playing' : formatSource(featured.source)}
                </span>
                {featuredLogo && !logoFailed ? (
                  <ArtMedia
                    className="home-hero-logo"
                    src={featuredLogo}
                    mime={featuredLogoMime}
                    alt={featured.name}
                    onError={() => setLogoFailed(true)}
                  />
                ) : (
                  <h1 className="home-hero-name">{featured.name}</h1>
                )}
                <p className="home-hero-meta">
                  {formatSource(featured.source)}
                  {featuredPlay ? ` · ${featuredPlay} played` : ''}
                  {` · ${featured.exe}`}
                </p>
                <p className="home-hero-lede">
                  Launch from the hub, attach the overlay when the game is live,
                  and keep your library one scroll away.
                </p>
                <div className="home-hero-actions">
                  {featured.running && featured.pid ? (
                    <motion.button
                      type="button"
                      className="btn-primary"
                      disabled={launching === String(featured.pid)}
                      onClick={() => onAttach(featured)}
                      whileHover={{ scale: 1.02 }}
                      whileTap={{ scale: 0.96 }}
                    >
                      {launching === String(featured.pid)
                        ? 'Attaching…'
                        : 'Attach Overlay'}
                    </motion.button>
                  ) : (
                    <HomePlayButton
                      gameId={featured.id}
                      launching={launching === featured.id}
                      blocked={prepareBlocksLaunch(featuredPrepare)}
                      onPlay={() => onPlay(featured)}
                    />
                  )}
                  {featuredFailed && !featured.running && (
                    <PrepareFailedBanner
                      prepare={featuredPrepare}
                      game={featured}
                      errorClassName="error-banner"
                      onRetry={onRetryPrepare}
                    />
                  )}
                  <button
                    type="button"
                    className="btn-secondary"
                    disabled={featured.running || featuredCreating}
                    onClick={() => onRetryPrepare(featured, undefined, true)}
                  >
                    {featuredCreating ? 'Installing…' : 'Install GSE'}
                  </button>
                  <button
                    type="button"
                    className={`btn-ghost${isFavorite(featured.id) ? ' is-favorite' : ''}`}
                    onClick={() => onToggleFavorite(featured.id)}
                  >
                    {isFavorite(featured.id) ? '★ Favorited' : '☆ Add to Favorite'}
                  </button>
                  {onCustomizeArt && (
                    <ArtGearButton onClick={() => onCustomizeArt(featured)} />
                  )}
                </div>
                {featuredCreating && !featured.running && (
                  <PrepareProgress forceGse={forcingGse} />
                )}
              </>
            ) : (
              <div className="home-empty">
                <h1 className="home-hero-name">Your library is quiet</h1>
                <p className="home-hero-lede">
                  Scan or add a game exe to light up this stage.
                </p>
                <button type="button" className="btn-primary" onClick={onOpenLibrary}>
                  Open library
                </button>
              </div>
            )}
          </div>

          <div className="home-side">
            <div className="home-game-rail glass">
              {railGames.length === 0 ? (
                <p className="home-muted">No games yet.</p>
              ) : (
                railGames.map((game) => {
                  const icon = covers[game.id]?.icon ?? covers[game.id]?.grid;
                  const { hue, initials } = gameAccent(game.name);
                  const active = game.id === featured?.id;
                  return (
                    <button
                      key={game.id}
                      type="button"
                      className={`home-rail-card${active ? ' active' : ''}`}
                      onClick={() => onSelectGame(game)}
                    >
                      <div
                        className="home-rail-thumb"
                        style={
                          icon
                            ? undefined
                            : {
                                background: `linear-gradient(160deg, hsl(${hue} 36% 28%), hsl(${hue} 22% 12%))`,
                              }
                        }
                      >
                        {icon ? (
                          <img src={icon} alt="" />
                        ) : (
                          <span>{initials}</span>
                        )}
                      </div>
                      <div className="home-rail-meta">
                        <span className="home-rail-tag">
                          {game.running
                            ? 'Now Playing'
                            : formatSource(game.source)}
                        </span>
                        <strong>{game.name}</strong>
                      </div>
                    </button>
                  );
                })
              )}
              <button
                type="button"
                className="home-rail-more"
                onClick={onOpenLibrary}
              >
                Discover more →
              </button>
            </div>
            <FriendRail onSelectFriend={onSelectFriend} />
          </div>
        </div>

        <div className="home-scroll-hint" aria-hidden>
          <span>↓</span>
        </div>
      </section>

      <div className="home-below">
        <section className="home-section">
          <div className="home-section-head">
            <h2>News</h2>
            <button type="button" className="text-link" onClick={onOpenStore}>
              Read more
            </button>
          </div>
          <div className="news-row">
            {newsCards.map((card, i) => (
              <motion.article
                key={card.key}
                className="news-card glass"
                initial={{ opacity: 0, y: 12 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true, margin: '-40px' }}
                transition={{ delay: i * 0.05, duration: 0.28 }}
              >
                <span className="news-tag">{card.tag}</span>
                <h3>{card.title}</h3>
                <p>{card.blurb}</p>
              </motion.article>
            ))}
          </div>
        </section>

        <section className="home-section">
          <div className="home-section-head">
            <h2>Discover</h2>
            <button type="button" className="text-link" onClick={onOpenLibrary}>
              See more games
            </button>
          </div>
          <div className="discover-grid">
            {discoverGames.length === 0 && catalogApps.length === 0 ? (
              <p className="home-muted">Add games to fill Discover.</p>
            ) : (
              <>
                {discoverGames.map((game, i) => {
                  const art = covers[game.id];
                  const src = art?.grid ?? art?.hero ?? art?.icon;
                  const { hue, initials } = gameAccent(game.name);
                  return (
                    <motion.button
                      key={game.id}
                      type="button"
                      className="discover-card"
                      onClick={() => onSelectGame(game)}
                      initial={{ opacity: 0, y: 14 }}
                      whileInView={{ opacity: 1, y: 0 }}
                      viewport={{ once: true }}
                      transition={{ delay: i * 0.04, duration: 0.28 }}
                      style={
                        src
                          ? undefined
                          : {
                              background: `linear-gradient(165deg, hsl(${hue} 38% 26%), #0a0c12)`,
                            }
                      }
                    >
                      {src && <img src={src} alt="" />}
                      {!src && <span className="discover-fallback">{initials}</span>}
                      <div className="discover-label">
                        <strong>{game.name}</strong>
                        <em>{formatSource(game.source)}</em>
                      </div>
                    </motion.button>
                  );
                })}
                {catalogApps.map((app, i) => (
                  <motion.button
                    key={`cat-${app.id}`}
                    type="button"
                    className="discover-card discover-card-store"
                    onClick={onOpenStore}
                    initial={{ opacity: 0, y: 14 }}
                    whileInView={{ opacity: 1, y: 0 }}
                    viewport={{ once: true }}
                    transition={{
                      delay: (discoverGames.length + i) * 0.04,
                      duration: 0.28,
                    }}
                  >
                    <div className="discover-label">
                      <span className="pill">Store</span>
                      <strong>{app.name}</strong>
                      <em>Overlay app · v{app.version}</em>
                    </div>
                  </motion.button>
                ))}
              </>
            )}
          </div>
        </section>

        <section className="home-section">
          <div className="home-section-head">
            <h2>Achievements</h2>
            <button type="button" className="text-link" onClick={onOpenAchievements}>
              Open gallery
            </button>
          </div>
          <div className="achievements-teaser glass">
            {teaserGame ? (
              <>
                <div className="achievements-teaser-hero">
                  <ProgressRing pct={pct} size={88} sublabel="complete" />
                  <div>
                    <p className="page-eyebrow">{teaserGame.name}</p>
                    <h3>
                      {total > 0
                        ? `${unlocked} of ${total} unlocked`
                        : 'No schema yet — open Achievements to set up tracking'}
                    </h3>
                    <div className="progress-bar achievements-teaser-bar">
                      <motion.div
                        className="progress-fill"
                        initial={{ width: 0 }}
                        animate={{ width: `${pct}%` }}
                        transition={{ duration: 0.6, ease: 'easeOut' }}
                      />
                    </div>
                  </div>
                </div>
                <div className="achievements-teaser-grid">
                  {recentUnlocks.length > 0
                    ? recentUnlocks.map((row) => (
                        <div key={row.achievement_id} className="trophy-chip on">
                          {row.icon_unlocked ? (
                            <img src={row.icon_unlocked} alt="" />
                          ) : (
                            <span className="achievements-dot" />
                          )}
                          <strong>{row.title}</strong>
                        </div>
                      ))
                    : [0, 1, 2, 3].map((n) => (
                        <div key={n} className="trophy-chip locked">
                          <span className="achievements-dot" />
                          <strong>Locked slot</strong>
                        </div>
                      ))}
                </div>
              </>
            ) : (
              <p className="home-muted">Add a library game to track trophies.</p>
            )}
          </div>
        </section>

        <section className="home-section">
          <div className="home-section-head">
            <h2>Social</h2>
          </div>
          <div className="social-grid">
            {mockSocialPosts.map((post, i) => (
              <motion.article
                key={post.id}
                className={`social-card glass${post.kind === 'image' ? ' social-card-image' : ''}`}
                initial={{ opacity: 0, y: 12 }}
                whileInView={{ opacity: 1, y: 0 }}
                viewport={{ once: true }}
                transition={{ delay: i * 0.05, duration: 0.28 }}
              >
                <div className="social-card-head">
                  <span
                    className="friend-avatar"
                    style={{ background: post.avatarColor }}
                  >
                    {post.author.slice(0, 2).toUpperCase()}
                  </span>
                  <div>
                    <strong>{post.author}</strong>
                    <em>{post.meta}</em>
                  </div>
                </div>
                <h3>{post.title}</h3>
                <p>{post.body}</p>
              </motion.article>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
