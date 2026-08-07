import type { GameArt, Profile, ScannedGame } from '../launcher-bridge';
import { WindowControls } from './WindowControls';

type Props = {
  profile: Profile | null;
  favorites?: ScannedGame[];
  covers?: Record<string, GameArt>;
  onSelectFavorite?: (game: ScannedGame) => void;
};

export function TopBar({
  profile,
  favorites = [],
  covers = {},
  onSelectFavorite,
}: Props) {
  const initials = (profile?.display_name ?? 'P').slice(0, 2).toUpperCase();
  const favs = favorites.slice(0, 5);

  return (
    <header className="topbar titlebar-drag">
      <div className="topbar-spacer" />
      <div className="topbar-pills">
        {favs.length > 0 && (
          <div className="glass-pill favorites-pill">
            <span className="favorites-star" aria-hidden>
              ★
            </span>
            {favs.map((game) => {
              const art = covers[game.id];
              const src = art?.icon ?? art?.grid;
              return (
                <button
                  key={game.id}
                  type="button"
                  className="fav-orb"
                  title={game.name}
                  onClick={() => onSelectFavorite?.(game)}
                >
                  {src ? (
                    <img src={src} alt="" width={28} height={28} />
                  ) : (
                    <span>{game.name.slice(0, 1)}</span>
                  )}
                </button>
              );
            })}
          </div>
        )}
        <div className="glass-pill profile-pill">
          <div className="profile-avatar">{initials}</div>
          <div className="profile-copy">
            <div className="profile-name">
              {profile?.display_name ?? profile?.username ?? 'Player'}
            </div>
            <div className="profile-status">Online</div>
          </div>
        </div>
        <WindowControls />
      </div>
    </header>
  );
}
