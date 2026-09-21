import { useEffect, useState } from 'react';
import type {
  MetricsDetailLevel,
  MetricsPrefs,
  MetricsSnapshot,
  MetricsTiles,
  PluginAPI,
} from '@glint/plugin-sdk';
import { defaultMetrics, frameGenLabel } from '@glint/plugin-sdk';

type TileId = keyof MetricsTiles;

const LEVELS: { id: MetricsDetailLevel; label: string }[] = [
  { id: 'classic', label: 'Classic' },
  { id: 'util', label: 'Util' },
  { id: 'full', label: 'Full' },
];

const LEVEL_TILES: Record<MetricsDetailLevel, TileId[]> = {
  classic: ['fps'],
  util: ['fps', 'cpu', 'gpu'],
  full: ['fps', 'cpu', 'gpu', 'ram', 'vram', 'graph'],
};

const TILE_LABELS: Record<TileId, string> = {
  fps: 'FPS',
  cpu: 'CPU',
  gpu: 'GPU',
  ram: 'RAM',
  vram: 'VRAM',
  graph: 'Graph',
};

const LEVEL_DEFAULTS: Record<MetricsDetailLevel, MetricsTiles> = {
  classic: { fps: true, cpu: false, gpu: false, ram: false, vram: false, graph: false },
  util: { fps: true, cpu: true, gpu: true, ram: false, vram: false, graph: false },
  full: { fps: true, cpu: true, gpu: true, ram: true, vram: true, graph: false },
};

const DEFAULT_PREFS: MetricsPrefs = {
  detailLevel: 'classic',
  tiles: LEVEL_DEFAULTS.classic,
};

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

function mergePrefs(base: MetricsPrefs, patch?: Partial<MetricsPrefs>): MetricsPrefs {
  return {
    detailLevel: patch?.detailLevel ?? base.detailLevel,
    tiles: { ...base.tiles, ...patch?.tiles },
  };
}

function tileEnabled(prefs: MetricsPrefs, id: TileId): boolean {
  return LEVEL_TILES[prefs.detailLevel].includes(id) && prefs.tiles[id] === true;
}

function hasHw(value: number | undefined): value is number {
  return typeof value === 'number';
}

function formatRam(used?: number, total?: number): string | null {
  if (!hasHw(used)) return null;
  if (!hasHw(total)) return `${used.toFixed(0)} MB`;
  return `${used.toFixed(0)} / ${total.toFixed(0)} MB`;
}

function formatVram(dedicated?: number, shared?: number): string | null {
  if (hasHw(dedicated) && hasHw(shared)) return `${dedicated.toFixed(0)} / ${shared.toFixed(0)} MB`;
  if (hasHw(dedicated)) return `${dedicated.toFixed(0)} MB`;
  if (hasHw(shared)) return `${shared.toFixed(0)} MB shared`;
  return null;
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
    void api.native.metrics.getSnapshot().then(apply).catch(() => {});
    return api.game.onMetrics(apply);
  }, [api]);

  return { metrics, history };
}

function usePrefs(api: PluginAPI) {
  const [prefs, setPrefs] = useState<MetricsPrefs>(DEFAULT_PREFS);

  useEffect(() => {
    void api.native.metrics.getPrefs().then((next) => {
      if (next?.detailLevel) setPrefs(mergePrefs(DEFAULT_PREFS, next));
    }).catch(() => {});
  }, [api]);

  const save = (patch: Partial<MetricsPrefs>) => {
    setPrefs((prev) => {
      if (patch.detailLevel && patch.detailLevel !== prev.detailLevel) {
        return mergePrefs({ detailLevel: patch.detailLevel, tiles: LEVEL_DEFAULTS[patch.detailLevel] }, patch);
      }
      return mergePrefs(prev, patch);
    });
    void api.native.metrics.setPrefs(patch).then((next) => {
      if (next?.detailLevel) setPrefs(mergePrefs(DEFAULT_PREFS, next));
    }).catch(() => {});
  };

  return { prefs, save };
}

function FpsRows({
  metrics,
  rowClass,
  labelClass,
  valueClass,
  digits,
}: {
  metrics: MetricsSnapshot;
  rowClass: string;
  labelClass: string;
  valueClass: string;
  digits: number;
}) {
  if (metrics.frameGenActive) {
    const label = frameGenLabel(metrics.frameGenKind) || 'FG';
    return (
      <>
        <div className={rowClass}>
          <span className={labelClass}>{label}</span>
          <span className={`${valueClass} generated`}>{metrics.generatedFps.toFixed(digits)}</span>
        </div>
        <div className={rowClass}>
          <span className={labelClass}>FPS</span>
          <span className={`${valueClass} native`}>{metrics.nativeFps.toFixed(digits)}</span>
        </div>
      </>
    );
  }
  return (
    <div className={rowClass}>
      <span className={labelClass}>FPS</span>
      <span className={`${valueClass} native`}>{metrics.generatedFps.toFixed(digits)}</span>
    </div>
  );
}

function HwRows({
  metrics,
  prefs,
  rowClass,
  labelClass,
}: {
  metrics: MetricsSnapshot;
  prefs: MetricsPrefs;
  rowClass: string;
  labelClass: string;
}) {
  const ram = formatRam(metrics.ramUsedMb, metrics.ramTotalMb);
  const vram = formatVram(metrics.vramDedicatedMb, metrics.vramSharedMb);
  return (
    <>
      {tileEnabled(prefs, 'cpu') && hasHw(metrics.cpuUtil) && (
        <div className={rowClass}>
          <span className={labelClass}>CPU</span>
          <span>{metrics.cpuUtil.toFixed(0)}%</span>
        </div>
      )}
      {tileEnabled(prefs, 'gpu') && hasHw(metrics.gpuUtil) && (
        <div className={rowClass}>
          <span className={labelClass}>GPU</span>
          <span>{metrics.gpuUtil.toFixed(0)}%</span>
        </div>
      )}
      {tileEnabled(prefs, 'ram') && ram && (
        <div className={rowClass}>
          <span className={labelClass}>RAM</span>
          <span>{ram}</span>
        </div>
      )}
      {tileEnabled(prefs, 'vram') && vram && (
        <div className={rowClass}>
          <span className={labelClass}>VRAM</span>
          <span>{vram}</span>
        </div>
      )}
    </>
  );
}

function FullMetrics({ api }: { api: PluginAPI }) {
  const { metrics, history } = useMetrics(api);
  const { prefs, save } = usePrefs(api);
  const pinned = api.panels.isPinned();
  const showFps = tileEnabled(prefs, 'fps');
  const showGraph = tileEnabled(prefs, 'graph');

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

      <div className="level-row" role="radiogroup" aria-label="Detail level">
        {LEVELS.map((level) => (
          <button
            key={level.id}
            type="button"
            role="radio"
            aria-checked={prefs.detailLevel === level.id}
            className={prefs.detailLevel === level.id ? 'pin-btn active' : 'pin-btn'}
            onClick={() => save({ detailLevel: level.id })}
          >
            {level.label}
          </button>
        ))}
      </div>

      <div className="tile-toggles">
        {LEVEL_TILES[prefs.detailLevel].map((id) => (
          <label key={id} className="tile-toggle">
            <input
              type="checkbox"
              checked={prefs.tiles[id] === true}
              onChange={(e) => save({ tiles: { [id]: e.target.checked } })}
            />
            {TILE_LABELS[id]}
          </label>
        ))}
      </div>

      <div className="metric-card">
        {showFps && (
          <FpsRows
            metrics={metrics}
            rowClass="metric-row"
            labelClass=""
            valueClass="metric-value"
            digits={1}
          />
        )}
        <HwRows
          metrics={metrics}
          prefs={prefs}
          rowClass="metric-row"
          labelClass="muted"
        />
      </div>

      {showFps && metrics.nativeFps === 0 && metrics.generatedFps === 0 && (
        <p className="muted" style={{ marginTop: '0.5rem' }}>
          No FPS data yet — waiting for Present-hook SHM. Admin/ETW is only for optional display split.
        </p>
      )}

      {showGraph && (
        <>
          <MiniChart values={history.native} variant="native" />
          <MiniChart values={history.generated} variant="generated" />
        </>
      )}
    </section>
  );
}

function HudMetrics({ api }: { api: PluginAPI }) {
  const { metrics, history } = useMetrics(api);
  const { prefs } = usePrefs(api);
  const showFps = tileEnabled(prefs, 'fps');
  const showGraph = tileEnabled(prefs, 'graph');

  return (
    <div className="metrics-app-root px-2 py-1" aria-label="Pinned FPS">
      {showFps && (
        <FpsRows
          metrics={metrics}
          rowClass="pinned-metrics-row"
          labelClass="pinned-label"
          valueClass="pinned-value"
          digits={0}
        />
      )}
      <HwRows
        metrics={metrics}
        prefs={prefs}
        rowClass="pinned-metrics-row"
        labelClass="pinned-label"
      />
      {showGraph && (
        <>
          <MiniChart values={history.native} variant="native" />
          <MiniChart values={history.generated} variant="generated" />
        </>
      )}
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
