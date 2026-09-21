import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import { flushSync } from 'react-dom';
import { hostInvoke } from '@glint/overlay-bridge';
import {
  persistOverlayLayout,
  type WindowBounds,
} from '../hooks/useOverlayWindows';

type Edge = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
const HANDLES: Edge[] = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];

/** Fired when an AppWindow move or resize commits (not during titlebar dest). */
export const OVERLAY_WINDOW_MOVED = 'overlay-window-moved';

type Props = {
  title: string;
  bounds: WindowBounds;
  zIndex: number;
  minimized: boolean;
  focused: boolean;
  privileged?: boolean;
  minWidth: number;
  minHeight: number;
  maxWidth: () => number;
  maxHeight: () => number;
  onFocus: () => void;
  onClose: () => void;
  onMinimize: () => void;
  onBoundsChange: (bounds: WindowBounds) => void;
  children: ReactNode;
};

type MoveDrag = {
  kind: 'move';
  startX: number;
  startY: number;
  /** `left`/`top` relative to `.overlay-window-layer`. */
  startBounds: WindowBounds;
  /** Layer top-left in the CEF atlas / viewport. */
  atlasOrigin: { x: number; y: number };
};

type ResizeDrag = {
  kind: 'resize';
  edge: Edge;
  startX: number;
  startY: number;
  startBounds: WindowBounds;
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(value)));
}

/** Two frames so CEF OSR can paint the isolated atlas before crop. */
function afterPaint(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => resolve());
    });
  });
}

function layerAtlasOrigin(el: HTMLElement | null): { x: number; y: number } {
  const parent = el?.offsetParent;
  if (!(parent instanceof HTMLElement)) return { x: 0, y: 0 };
  const rect = parent.getBoundingClientRect();
  return { x: Math.round(rect.left), y: Math.round(rect.top) };
}

function toAtlas(
  layout: { x: number; y: number },
  origin: { x: number; y: number },
): [number, number] {
  return [layout.x + origin.x, layout.y + origin.y];
}

export function AppWindow({
  title,
  bounds,
  zIndex,
  minimized,
  focused,
  privileged,
  minWidth,
  minHeight,
  maxWidth,
  maxHeight,
  onFocus,
  onClose,
  onMinimize,
  onBoundsChange,
  children,
}: Props) {
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;
  const elRef = useRef<HTMLDivElement>(null);
  const [promoted, setPromoted] = useState(false);
  const promoteGen = useRef(0);

  const dragRef = useRef<MoveDrag | ResizeDrag | null>(null);

  const applyBounds = useCallback(
    (next: WindowBounds) => {
      const clamped: WindowBounds = {
        x: clamp(next.x, 0, window.innerWidth - minWidth),
        y: clamp(next.y, 0, window.innerHeight - minHeight - 88),
        width: clamp(next.width, minWidth, maxWidth()),
        height: clamp(next.height, minHeight, maxHeight()),
      };
      onBoundsChange(clamped);
    },
    [minWidth, minHeight, maxWidth, maxHeight, onBoundsChange],
  );

  useEffect(() => {
    const onMove = (event: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;

      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      const start = drag.startBounds;

      if (drag.kind === 'move') {
        const x = clamp(start.x + dx, 0, window.innerWidth - minWidth);
        const y = clamp(start.y + dy, 0, window.innerHeight - minHeight - 88);
        void hostInvoke('native.overlay.setPosition', toAtlas({ x, y }, drag.atlasOrigin));
        return;
      }

      let { x, y, width, height } = start;
      const edge = drag.edge;

      if (edge.includes('e')) width = start.width + dx;
      if (edge.includes('w')) {
        width = start.width - dx;
        x = start.x + dx;
      }
      if (edge.includes('s')) height = start.height + dy;
      if (edge.includes('n')) {
        height = start.height - dy;
        y = start.y + dy;
      }

      const clampedWidth = clamp(width, minWidth, maxWidth());
      const clampedHeight = clamp(height, minHeight, maxHeight());

      if (edge.includes('w') && clampedWidth !== width) {
        x += width - clampedWidth;
      }
      if (edge.includes('n') && clampedHeight !== height) {
        y += height - clampedHeight;
      }

      applyBounds({ x, y, width: clampedWidth, height: clampedHeight });
    };

    const onUp = (event: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      dragRef.current = null;
      document.body.classList.remove('overlay-window-dragging');
      document.body.classList.remove('overlay-window-moving');
      document.body.classList.remove('overlay-promoting');
      void hostInvoke('native.overlay.setShellDrag', [false]);
      try {
        (event.target as Element | null)?.releasePointerCapture?.(event.pointerId);
      } catch {
        /* already released */
      }

      if (drag.kind === 'move') {
        const dx = event.clientX - drag.startX;
        const dy = event.clientY - drag.startY;
        const start = drag.startBounds;
        const next = {
          ...start,
          x: clamp(start.x + dx, 0, window.innerWidth - minWidth),
          y: clamp(start.y + dy, 0, window.innerHeight - minHeight - 88),
        };
        void (async () => {
          await hostInvoke(
            'native.overlay.setPosition',
            toAtlas(next, drag.atlasOrigin),
          );
          flushSync(() => {
            if (elRef.current) {
              elRef.current.style.visibility = '';
              elRef.current.classList.remove('is-promoting');
            }
            setPromoted(false);
          });
          applyBounds(next);
          await hostInvoke('native.overlay.setPosition', []);
          persistOverlayLayout();
          window.dispatchEvent(new Event(OVERLAY_WINDOW_MOVED));
        })();
        return;
      }

      persistOverlayLayout();
    };

    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.addEventListener('pointercancel', onUp);
    return () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.removeEventListener('pointercancel', onUp);
    };
  }, [applyBounds, minWidth, minHeight, maxWidth, maxHeight]);

  const startMove = (event: React.PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    onFocus();
    const start = boundsRef.current;
    const atlasOrigin = layerAtlasOrigin(elRef.current);
    dragRef.current = {
      kind: 'move',
      startX: event.clientX,
      startY: event.clientY,
      startBounds: start,
      atlasOrigin,
    };
    document.body.classList.add('overlay-window-dragging');
    document.body.classList.add('overlay-window-moving');
    void hostInvoke('native.overlay.setShellDrag', [true]);
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
    const gen = ++promoteGen.current;
    flushSync(() => {
      document.body.classList.add('overlay-promoting');
      elRef.current?.classList.add('is-promoting');
    });
    void (async () => {
      await afterPaint();
      if (dragRef.current?.kind !== 'move' || promoteGen.current !== gen) return;
      const [ax, ay] = toAtlas(start, atlasOrigin);
      await hostInvoke('native.overlay.setPosition', [
        ax,
        ay,
        start.width,
        start.height,
      ]);
      if (dragRef.current?.kind !== 'move' || promoteGen.current !== gen) return;
      flushSync(() => {
        if (elRef.current) {
          elRef.current.style.visibility = 'hidden';
          elRef.current.classList.remove('is-promoting');
        }
        document.body.classList.remove('overlay-promoting');
        setPromoted(true);
      });
    })();
  };

  const startResize = (edge: Edge) => (event: React.PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    onFocus();
    dragRef.current = {
      kind: 'resize',
      edge,
      startX: event.clientX,
      startY: event.clientY,
      startBounds: boundsRef.current,
    };
    document.body.classList.add('overlay-window-dragging');
    void hostInvoke('native.overlay.setShellDrag', [true]);
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
  };

  if (minimized) return null;

  const style: CSSProperties = {
    left: bounds.x,
    top: bounds.y,
    width: bounds.width,
    height: bounds.height,
    zIndex,
    visibility: promoted ? 'hidden' : undefined,
  };

  return (
    <div
      ref={elRef}
      className={[
        'overlay-app-window',
        focused ? 'focused' : '',
        privileged ? 'privileged' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      style={style}
      onMouseDown={onFocus}
    >
      <header className="overlay-window-titlebar" onPointerDown={startMove}>
        <span className="overlay-window-title">{title}</span>
        <div className="overlay-window-controls">
          <button
            type="button"
            className="overlay-window-btn minimize"
            aria-label="Minimize"
            onPointerDown={(e) => e.stopPropagation()}
            onClick={onMinimize}
          >
            ─
          </button>
          <button
            type="button"
            className="overlay-window-btn close"
            aria-label="Close"
            onPointerDown={(e) => e.stopPropagation()}
            onClick={onClose}
          >
            ✕
          </button>
        </div>
      </header>
      <div className="overlay-window-body">{children}</div>
      {HANDLES.map((edge) => (
        <div
          key={edge}
          className={`overlay-resize-handle overlay-resize-${edge}`}
          onPointerDown={startResize(edge)}
          aria-hidden
        />
      ))}
    </div>
  );
}
