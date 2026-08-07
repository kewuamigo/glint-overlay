import { useCallback, useEffect, useState } from 'react';
import type { PluginAPI, PluginRenderMode } from '@glint/plugin-sdk';
import { gameAccent } from './gameAccent';
import { ProgressRing } from './ProgressRing';

type TrackedGame = {
  id: string;
  name: string;
  platform: string;
  source: string;
};

type AchievementRow = {
  achievement_id: string;
  title: string;
  description: string | null;
  icon_unlocked: string | null;
  unlocked: number;
};

type Props = {
  api: PluginAPI;
  mode: PluginRenderMode;
};

function achInvoke<T>(pluginId: string, method: string, args: unknown[] = []): Promise<T> {
  const bridge = window.__goHost;
  if (!bridge) return Promise.reject(new Error('host bridge unavailable'));
  return bridge.invoke<T>(method, args, pluginId);
}

export function AchievementsApp({ api, mode }: Props) {
  const [game, setGame] = useState<TrackedGame | null>(null);
  const [rows, setRows] = useState<AchievementRow[]>([]);
  const [status, setStatus] = useState<string | null>(null);
  const [toastTestPending, setToastTestPending] = useState(false);

  const bindToSession = useCallback(async () => {
    setStatus(null);
    try {
      const info = await api.game.getProcessInfo();
      const exePath = info.exePath;
      const exeName = info.exeName || exePath.split(/[/\\]/).pop() || '';
      if (!exePath) {
        setGame(null);
        setRows([]);
        setStatus('No game process path yet.');
        return;
      }

      // Keep tracking side-effects for the attached exe; ignore emu guide payload.
      await achInvoke(api.pluginId, 'achievements.trackExe', [exePath]).catch(() => undefined);

      let matched = await achInvoke<TrackedGame | null>(
        api.pluginId,
        'achievements.forProcess',
        [exePath, exeName],
      );
      if (!matched) {
        const games = await achInvoke<TrackedGame[]>(
          api.pluginId,
          'achievements.listGames',
        );
        matched =
          games.find((g) => g.id.includes(exeName.toLowerCase())) ?? null;
      }
      setGame(matched);
      if (matched) {
        const list = await achInvoke<AchievementRow[]>(
          api.pluginId,
          'achievements.listForGame',
          [matched.id],
        );
        setRows(Array.isArray(list) ? list : []);
      } else {
        setRows([]);
      }
    } catch (err) {
      setStatus(err instanceof Error ? err.message : 'Failed to bind game');
    }
  }, [api]);

  useEffect(() => {
    void bindToSession();
    const timer = window.setInterval(() => void bindToSession(), 4000);
    return () => window.clearInterval(timer);
  }, [bindToSession]);

  const unlocked = rows.filter((r) => r.unlocked).length;
  const pct = rows.length > 0 ? Math.round((unlocked / rows.length) * 100) : 0;
  const accent = game ? gameAccent(game.name) : null;
  const sortedRows = [...rows].sort((a, b) => Number(b.unlocked) - Number(a.unlocked));

  if (mode === 'hud') {
    return (
      <div className="ach-hud">
        <strong>{game?.name ?? 'Achievements'}</strong>
        <span>{game ? `${unlocked}/${rows.length}` : '…'}</span>
      </div>
    );
  }

  return (
    <div className="ach-root">
      <header className="ach-header">
        <div>
          <p className="ach-eyebrow">Trophy room</p>
          <h2>{game?.name ?? 'Achievements'}</h2>
          <p className="ach-sub">
            {game
              ? `${game.platform} · ${game.source}`
              : 'Bound to the current overlay game automatically'}
          </p>
        </div>
        <button
          type="button"
          className="ach-btn"
          disabled={toastTestPending}
          onClick={() => {
            setToastTestPending(true);
            setStatus('Replaying unlocked toasts in 5s — close overlay (Shift+Tab)');
            window.setTimeout(() => {
              void achInvoke(api.pluginId, 'achievements.testToast', [])
                .catch((err) =>
                  setStatus(err instanceof Error ? err.message : 'Toast test failed'),
                )
                .finally(() => setToastTestPending(false));
            }, 5_000);
          }}
        >
          {toastTestPending ? 'Toast in 5s…' : 'Replay unlocks (5s)'}
        </button>
      </header>

      {status && <p className="ach-status">{status}</p>}

      {!game || !accent ? (
        <div className="ach-empty">
          Waiting for the attached game… Track exes from the launcher.
        </div>
      ) : (
        <>
          <div
            className="ach-showcase"
            style={{
              background: `linear-gradient(120deg, hsl(${accent.hue} 42% 20%) 0%, #07080c 70%)`,
            }}
          >
            <div className="ach-showcase-shade" />
            <div className="ach-showcase-body">
              <ProgressRing pct={pct} size={76} />
              <div>
                <span className="ach-pill">{game.source}</span>
                <h3>{game.name}</h3>
                <p>
                  {rows.length > 0
                    ? `${unlocked} / ${rows.length} unlocked`
                    : 'No achievement schema found yet'}
                </p>
                <div className="ach-progress-bar">
                  <div
                    className="ach-progress-fill"
                    style={{ width: `${pct}%` }}
                  />
                </div>
              </div>
            </div>
          </div>

          {rows.length === 0 ? (
            <div className="ach-empty">
              No trophies for this title yet. Add a schema next to the exe, then reopen.
            </div>
          ) : (
            <div className="ach-trophy-grid">
              {sortedRows.map((r) => (
                <article
                  key={r.achievement_id}
                  className={`ach-trophy-card${r.unlocked ? ' on' : ' locked'}`}
                >
                  {r.icon_unlocked ? (
                    <img src={r.icon_unlocked} alt="" />
                  ) : (
                    <span className="ach-dot" />
                  )}
                  <div>
                    <strong>{r.title}</strong>
                    {r.description && <p>{r.description}</p>}
                    <em>{r.unlocked ? 'Unlocked' : 'Locked'}</em>
                  </div>
                </article>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}
