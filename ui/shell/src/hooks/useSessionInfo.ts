import { useEffect, useRef, useState } from 'react';
import { useOverlay } from '../context/OverlayContext';

const CLOCK_TICK_MS = 30_000;

function pad2(value: number): string {
  return String(value).padStart(2, '0');
}

/** Local wall clock, HH:MM (24 h). */
export function formatClock(date: Date): string {
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

/** Elapsed since sessionStartMs, minute precision ("1ó 12p" / "12p"). */
export function formatElapsed(startMs: number, nowMs: number): string {
  const minutes = Math.max(0, Math.floor((nowMs - startMs) / 60_000));
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return hours > 0 ? `${hours}ó ${rest}p` : `${rest}p`;
}

/** Mirrors launcher `formatPlaytime` ("12.4 h" / "42 m"); null hides the chip. */
export function formatTotal(seconds: number | null): string | null {
  if (seconds == null || seconds <= 0) return null;
  const hours = seconds / 3600;
  if (hours < 1) {
    const mins = Math.max(1, Math.round(hours * 60));
    return `${mins} m`;
  }
  return `${hours.toFixed(hours < 10 ? 1 : 0)} h`;
}

export function useSessionInfo() {
  const { connected, gameName, playtimeSeconds, attachedAtMs } = useOverlay();
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), CLOCK_TICK_MS);
    return () => window.clearInterval(id);
  }, []);

  // Session anchored at attach epoch (server-side), survives unmount/reload.
  const sessionStartMs = attachedAtMs;

  return {
    clock: formatClock(now),
    session:
      connected && sessionStartMs != null
        ? formatElapsed(sessionStartMs, now.getTime())
        : null,
    total: formatTotal(playtimeSeconds),
    gameName,
  };
}
