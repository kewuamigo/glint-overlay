type Props = {
  pct: number;
  size?: number;
  label?: string;
  sublabel?: string;
};

/** Smooth SVG progress ring — avoids broken conic-gradient arcs. */
export function ProgressRing({
  pct,
  size = 84,
  label,
  sublabel = 'done',
}: Props) {
  const clamped = Math.max(0, Math.min(100, pct));
  const stroke = 5;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const offset = c * (1 - clamped / 100);
  const display = label ?? `${Math.round(clamped)}%`;

  return (
    <div
      className="progress-ring"
      style={{ width: size, height: size }}
      role="img"
      aria-label={`${Math.round(clamped)} percent complete`}
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <circle
          className="progress-ring-track"
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          strokeWidth={stroke}
        />
        <circle
          className="progress-ring-value"
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          strokeWidth={stroke}
          strokeDasharray={c}
          strokeDashoffset={offset}
          strokeLinecap="round"
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
        />
      </svg>
      <div className="progress-ring-label">
        <strong>{display}</strong>
        {sublabel ? <span>{sublabel}</span> : null}
      </div>
    </div>
  );
}
