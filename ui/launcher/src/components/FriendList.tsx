import { useEffect, useRef, useState } from 'react';
import { mockFriends, type MockFriend } from '../data/mockFriends';
import { launcherInvoke } from '../launcher-bridge';

type Props = {
  onSelectFriend: (friend: MockFriend) => void;
};

function statusLabel(friend: MockFriend): string {
  if (friend.game) return `Playing ${friend.game}`;
  if (friend.status === 'online') return 'Online';
  if (friend.status === 'away') return 'Away';
  return 'Offline';
}

export function FriendList({ onSelectFriend }: Props) {
  return (
    <div className="friends-section">
      <div className="friends-title">Friend Activity</div>
      <div className="friends-scroll">
        {mockFriends.map((friend) => (
          <button
            key={friend.name}
            type="button"
            className="friend-row"
            onClick={() => onSelectFriend(friend)}
          >
            <div
              className="friend-avatar"
              style={{ background: friend.avatarColor }}
            >
              {friend.name.slice(0, 2).toUpperCase()}
              <span className={`friend-status-dot ${friend.status}`} />
            </div>
            <div className="friend-info">
              <div className="friend-name">{friend.name}</div>
              <div className="friend-game">{statusLabel(friend)}</div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

export function FriendRail({ onSelectFriend }: Props) {
  const [hovered, setHovered] = useState<string | null>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const open = (name: string) => {
    if (closeTimer.current) {
      clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
    setHovered(name);
  };

  const scheduleClose = () => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
    closeTimer.current = setTimeout(() => {
      setHovered(null);
      closeTimer.current = null;
    }, 160);
  };

  useEffect(
    () => () => {
      if (closeTimer.current) clearTimeout(closeTimer.current);
    },
    [],
  );

  return (
    <aside className="friend-rail" aria-label="Friends">
      {mockFriends.map((friend) => {
        const isOpen = hovered === friend.name;
        return (
          <div
            key={friend.name}
            className={`friend-rail-item${isOpen ? ' open' : ''}`}
            onMouseEnter={() => open(friend.name)}
            onMouseLeave={scheduleClose}
          >
            <button
              type="button"
              className="friend-rail-orb"
              aria-label={`Open ${friend.name}'s profile`}
              onClick={() => onSelectFriend(friend)}
            >
              <span
                className="friend-rail-avatar"
                style={{ background: friend.avatarColor }}
              >
                {friend.name.slice(0, 2).toUpperCase()}
              </span>
              <span className={`friend-status-dot ${friend.status}`} />
            </button>

            <div
              className="friend-hover-card glass"
              role="dialog"
              aria-hidden={!isOpen}
            >
              <div className="friend-hover-head">
                <span
                  className="friend-hover-avatar"
                  style={{ background: friend.avatarColor }}
                >
                  {friend.name.slice(0, 2).toUpperCase()}
                  <span className={`friend-status-dot ${friend.status}`} />
                </span>
                <div>
                  <strong>{friend.name}</strong>
                  <em className={`friend-hover-status ${friend.status}`}>
                    {statusLabel(friend)}
                  </em>
                </div>
              </div>
              <p className="friend-hover-bio">{friend.bio}</p>
              <div className="friend-hover-stats">
                <span>
                  <strong>{friend.level}</strong>
                  Level
                </span>
                <span>
                  <strong>{friend.achievements.length}</strong>
                  Trophies
                </span>
                <span>
                  <strong>
                    {Math.round(
                      friend.achievements.reduce((s, a) => s + a.progress, 0) /
                        Math.max(friend.achievements.length, 1),
                    )}
                    %
                  </strong>
                  Avg
                </span>
              </div>
              {friend.achievements[0] && (
                <div className="friend-hover-trophy">
                  <span className="news-tag">Latest</span>
                  <strong>{friend.achievements[0].title}</strong>
                  <em>{friend.achievements[0].game}</em>
                </div>
              )}
              <button
                type="button"
                className="btn-primary friend-hover-cta"
                tabIndex={isOpen ? 0 : -1}
                onClick={() => onSelectFriend(friend)}
              >
                Open profile
              </button>
            </div>
          </div>
        );
      })}
    </aside>
  );
}

export function AddGameForm({ onAdd }: { onAdd: (name: string, exe: string) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pickFolder = async () => {
    setBusy(true);
    setError(null);
    try {
      const picked = await launcherInvoke<{
        exe: string;
        name: string;
      } | null>('games.pickFolder');
      if (!picked) return;
      onAdd(picked.name, picked.exe);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const pickExe = async () => {
    setBusy(true);
    setError(null);
    try {
      const exe = await launcherInvoke<string | null>('achievements.pickExe');
      if (!exe) return;
      const base = exe.split(/[/\\]/).pop() ?? 'game.exe';
      const name = base.replace(/\.exe$/i, '') || base;
      onAdd(name, exe);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        type="button"
        className="btn-secondary add-game-btn"
        disabled={busy}
        onClick={() => void pickFolder()}
      >
        {busy ? 'Browsing…' : '+ Add Game'}
      </button>
      <button
        type="button"
        className="btn-secondary"
        disabled={busy}
        onClick={() => void pickExe()}
      >
        Add exe
      </button>
      {error ? <span className="library-add-error">{error}</span> : null}
    </>
  );
}
