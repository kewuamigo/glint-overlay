export function WindowControls() {
  return (
    <div className="window-controls">
      <button
        type="button"
        className="window-btn"
        aria-label="Minimize"
        onClick={() => void window.__goLauncher?.invoke('window.minimize')}
      >
        ─
      </button>
      <button
        type="button"
        className="window-btn"
        aria-label="Maximize"
        onClick={() => void window.__goLauncher?.invoke('window.maximize')}
      >
        ▢
      </button>
      <button
        type="button"
        className="window-btn window-btn-close"
        aria-label="Close"
        onClick={() => void window.__goLauncher?.invoke('window.close')}
      >
        ✕
      </button>
    </div>
  );
}
