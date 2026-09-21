import { useEffect, useRef, useState } from 'react';
import { hostInvoke } from '@glint/overlay-bridge';

export type AchievementToastPayload = {
  gameName: string;
  title: string;
  iconUrl?: string;
  score?: number | string;
  rare?: boolean;
};

type ToastItem = AchievementToastPayload & { id: number };

// Relative to the shell document (file:// dist). Host also plays these via
// PlaySound; JS is a fallback when `__goHost` is missing (standalone preview).
const SOUND_URL = './achievements/XboxAchievement.wav';
const SOUND_RARE_URL = './achievements/XboxOneRareAchievement.wav';
const ANIM_MS = 12_000;

export function AchievementToasts() {
  const [current, setCurrent] = useState<ToastItem | null>(null);
  const [playing, setPlaying] = useState(false);
  const queueRef = useRef<ToastItem[]>([]);
  const busyRef = useRef(false);
  const soundRef = useRef<HTMLAudioElement | null>(null);
  const soundRareRef = useRef<HTMLAudioElement | null>(null);

  useEffect(() => {
    const sound = new Audio(SOUND_URL);
    sound.preload = 'auto';
    soundRef.current = sound;
    const soundRare = new Audio(SOUND_RARE_URL);
    soundRare.preload = 'auto';
    soundRareRef.current = soundRare;
    let timer: number | undefined;

    const playNext = () => {
      if (busyRef.current) return;
      const next = queueRef.current.shift();
      if (!next) {
        setCurrent(null);
        setPlaying(false);
        return;
      }
      busyRef.current = true;
      setCurrent(next);
      setPlaying(false);
      // Restart CSS animations on the next frame after mount/update.
      requestAnimationFrame(() => {
        setPlaying(true);
        const playHtml = () => {
          const a = next.rare ? soundRareRef.current : soundRef.current;
          if (a) {
            a.currentTime = 0;
            void a.play().catch(() => undefined);
          }
        };
        if (window.__goHost) {
          void hostInvoke('ui.playAchievementSound', [Boolean(next.rare)]).catch(
            playHtml,
          );
        } else {
          playHtml();
        }
      });
      timer = window.setTimeout(() => {
        busyRef.current = false;
        setPlaying(false);
        setCurrent(null);
        // Drain next after unmount so classes fully reset.
        requestAnimationFrame(() => playNext());
      }, ANIM_MS);
    };

    const handler = (event: MessageEvent) => {
      if (event.data?.type !== 'achievement') return;
      const scoreRaw = event.data.score;
      const score =
        typeof scoreRaw === 'number' || typeof scoreRaw === 'string'
          ? scoreRaw
          : undefined;
      const item: ToastItem = {
        id: Date.now() + Math.random(),
        gameName: String(event.data.gameName ?? 'Game'),
        title: String(event.data.title ?? 'Achievement unlocked'),
        iconUrl:
          typeof event.data.iconUrl === 'string' ? event.data.iconUrl : undefined,
        score,
        rare: Boolean(event.data.rare),
      };
      queueRef.current.push(item);
      playNext();
    };

    window.addEventListener('message', handler);
    return () => {
      window.removeEventListener('message', handler);
      if (timer !== undefined) window.clearTimeout(timer);
      busyRef.current = false;
      soundRef.current = null;
      soundRareRef.current = null;
    };
  }, []);

  if (!current) return null;

  const hasScore =
    current.score !== undefined &&
    current.score !== null &&
    String(current.score).length > 0;

  return (
    <div
      className={`achievement${current.rare ? ' rare' : ''}`}
      aria-live="polite"
    >
      <div className="animation">
        <div className={`circle${playing ? ' circle_animate' : ''}`}>
          <div className="img trophy_animate trophy_img">
            {current.iconUrl ? (
              <img className="achiev_icon" src={current.iconUrl} alt="" />
            ) : (
              <>
                <img
                  className="trophy_1"
                  src="/achievements/trophy_full.svg"
                  alt=""
                />
                <img
                  className="trophy_2"
                  src="/achievements/trophy_no_handles.svg"
                  alt=""
                />
              </>
            )}
          </div>
          {/* Bloom inside circle — same DOM as 85a8c32 (pre-polish). */}
          <div className="brilliant-wrap" aria-hidden>
            <div className="brilliant" />
          </div>
        </div>
        <div className="banner-outer">
          <div className={`banner${playing ? ' banner-animate' : ''}`}>
            <div
              className={`achieve_disp${playing ? ' achieve_disp_animate' : ''}`}
            >
              <span className="unlocked">
                {current.rare
                  ? 'Rare achievement unlocked'
                  : 'Achievement unlocked'}
              </span>
              <div className="score_disp">
                {hasScore ? (
                  <>
                    <div className="gamerscore">
                      <img width={20} src="/achievements/G.svg" alt="" />
                      <span className="acheive_score">{current.score}</span>
                    </div>
                    <span className="hyphen_sep">-</span>
                  </>
                ) : null}
                <span className="achiev_name">{current.title}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
