import { useEffect, useState } from 'react';
import type { MetricsSnapshot, PluginAPI } from '@glint/plugin-sdk';
import { defaultMetrics, frameGenLabel } from '@glint/plugin-sdk';

function MiniChart({
  values,
  variant,
}: {
  values: number[];
  variant: 'native' | 'generated';
}) {
  const max = Math.max(...values, 1);
  return (
    <div className="chart" aria-hidden>
      {values.map((v, i) => (
        <div
          key={i}
          className={`chart-bar ${variant}`}
          style={{ height: `${(v / max) * 100}%` }}
        />
      ))}
    </div>
  );
}

function formatMs(ms: number): string {
  if (ms <= 0) return '—';
  return `${ms.toFixed(1)} ms`;
}

function useMetrics(api: PluginAPI) {
  const [metrics, setMetrics] = useState<MetricsSnapshot>(() => ({ ...defaultMetrics }));
  const [history, setHistory] = useState<{ native: number[]; generated: number[] }>({
    native: [],
    generated: [],
  });

  useEffect(() => {
    const apply = (next: MetricsSnapshot) => {
      if (!next || typeof next.nativeFps !== 'number') return;
      setMetrics(next);
      setHistory((prev) => ({
        native: [...prev.native.slice(-89), next.nativeFps],
        generated: [...prev.generated.slice(-89), next.generatedFps],
      }));
    };
    // Push alone can miss the first ≤1 Hz tick / brief SHM gaps — pull once on mount.
    void api.native.metrics.getSnapshot().then(apply).catch(() => {});
    return api.game.onMetrics(apply);
  }, [api]);

  return { metrics, history };
}

function FullMetrics({ api }: { api: PluginAPI }) {
  const { metrics, history } = useMetrics(api);
  const pinned = api.panels.isPinned();

  return (
    <section className="metrics-app-root p-3" aria-label="FPS Metrics">
      <div className="panel-header-row">
        <div>
          <p className="panel-eyebrow">Telemetry</p>
          <h2>Performance</h2>
        </div>
        <button
          type="button"
          className={pinned ? 'pin-btn active' : 'pin-btn'}
          onClick={() => api.panels.setPinned(!pinned)}
          title="Pin compact FPS HUD (visible when overlay is closed)"
        >
          {pinned ? 'Pinned' : 'Pin mini'}
        </button>
      </div>

      <div className="metric-card">
      {/* Steam-style: generated frame rate labelled by its FG tech, real FPS below */}
      {metrics.frameGenActive && (
        <div className="metric-row">
          <span>{frameGenLabel(metrics.frameGenKind) || 'FG'}</span>
          <span className="metric-value generated">{metrics.generatedFps.toFixed(1)}</span>
        </div>
      )}
      <div className="metric-row">
        <span>FPS</span>
        <span className="metric-value native">{metrics.nativeFps.toFixed(1)}</span>
      </div>
      {!metrics.frameGenActive && (
        <div className="metric-row">
          <span className="muted">Display FPS</span>
          <span className="metric-value generated">{metrics.generatedFps.toFixed(1)}</span>
        </div>
      )}

      <div className="metric-row">
        <span className="muted">Native frametime</span>
        <span>{formatMs(metrics.nativeFrameTimeMs)}</span>
      </div>
      <div className="metric-row">
        <span className="muted">Display frametime</span>
        <span>{formatMs(metrics.displayFrameTimeMs)}</span>
      </div>
      </div>

      {metrics.frameGenActive && (
        <div style={{ marginTop: '0.5rem' }}>
          <span className="badge">
            {frameGenLabel(metrics.frameGenKind) || 'FG'} Frame Generation{' '}
            {metrics.frameGenRatio.toFixed(2)}x
          </span>
        </div>
      )}

      {metrics.nativeFps === 0 && metrics.generatedFps === 0 && (
        <p className="muted" style={{ marginTop: '0.5rem' }}>
          No FPS data yet — waiting for Present-hook SHM. Admin/ETW is only for optional display split.
        </p>
      )}

      {metrics.nativeFps > 0 && (
        <p className="muted" style={{ marginTop: '0.5rem' }}>
          {metrics.frameSplitSource === 'driver'
            ? 'Per-frame driver tags (PresentMon provider)'
            : metrics.frameSplitSource === 'hook'
              ? 'Native = Present hook · Display = ETW flip'
              : 'ETW heuristic split'}
          {metrics.frameGenActive ? ' · FrameGen active' : ''}
        </p>
      )}

      <div className="metric-row" style={{ marginTop: '0.75rem' }}>
        <span className="muted">Native min/max</span>
        <span>
          {metrics.nativeMin.toFixed(0)} / {metrics.nativeMax.toFixed(0)}
        </span>
      </div>
      <div className="metric-row">
        <span className="muted">Display min/max</span>
        <span>
          {metrics.generatedMin.toFixed(0)} / {metrics.generatedMax.toFixed(0)}
        </span>
      </div>

      <MiniChart values={history.native} variant="native" />
      <MiniChart values={history.generated} variant="generated" />
    </section>
  );
}

function HudMetrics({ api }: { api: PluginAPI }) {
  const { metrics } = useMetrics(api);

  return (
    <div className="metrics-app-root px-2 py-1" aria-label="Pinned FPS">
      {/* Steam-style: FG rate first (labelled), then real FPS */}
      {metrics.frameGenActive && (
        <div className="pinned-metrics-row">
          <span className="pinned-label">{frameGenLabel(metrics.frameGenKind) || 'FG'}</span>
          <span className="pinned-value generated">{metrics.generatedFps.toFixed(0)}</span>
        </div>
      )}
      <div className="pinned-metrics-row">
        <span className="pinned-label">FPS</span>
        <span className="pinned-value native">{metrics.nativeFps.toFixed(0)}</span>
      </div>
      {!metrics.frameGenActive && (
        <div className="pinned-metrics-row">
          <span className="pinned-label">Display</span>
          <span className="pinned-value generated">{metrics.generatedFps.toFixed(0)}</span>
        </div>
      )}
      {metrics.frameGenActive && (
        <div className="pinned-badge">
          {frameGenLabel(metrics.frameGenKind) || 'FG'} {metrics.frameGenRatio.toFixed(2)}x
        </div>
      )}
      <div className="pinned-metrics-row">
        <span className="pinned-label">Native FT</span>
        <span>{metrics.nativeFrameTimeMs > 0 ? `${metrics.nativeFrameTimeMs.toFixed(1)} ms` : '—'}</span>
      </div>
      <div className="pinned-metrics-row">
        <span className="pinned-label">Display FT</span>
        <span>{metrics.displayFrameTimeMs > 0 ? `${metrics.displayFrameTimeMs.toFixed(1)} ms` : '—'}</span>
      </div>
    </div>
  );
}

export function MetricsApp({
  api,
  mode,
}: {
  api: PluginAPI;
  mode: 'hud' | 'full';
}) {
  if (mode === 'hud') return <HudMetrics api={api} />;
  return <FullMetrics api={api} />;
}
