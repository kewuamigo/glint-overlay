import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  type CSSProperties,
  type ReactNode,
} from 'react';
import type { WindowBounds } from '../hooks/useOverlayWindows';

type Edge = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
const HANDLES: Edge[] = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];

/** Fired after layout when an AppWindow finished applying a move (not resize). */
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

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(value)));
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

  const dragRef = useRef<
    | { kind: 'move'; startX: number; startY: number; startBounds: WindowBounds }
    | {
        kind: 'resize';
        edge: Edge;
        startX: number;
        startY: number;
        startBounds: WindowBounds;
      }
    | null
  >(null);

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

  // ResizeObserver covers size sync for the CEF hole; position-only moves do
  // not resize the hole element, so notify after layout while moving.
  useLayoutEffect(() => {
    if (!document.body.classList.contains('overlay-window-moving')) return;
    window.dispatchEvent(new Event(OVERLAY_WINDOW_MOVED));
  }, [bounds.x, bounds.y]);

  useEffect(() => {
    const onMove = (event: MouseEvent) => {
      const drag = dragRef.current;
      if (!drag) return;

      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      const start = drag.startBounds;

      if (drag.kind === 'move') {
        applyBounds({
          ...start,
          x: start.x + dx,
          y: start.y + dy,
        });
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

    const onUp = () => {
      if (!dragRef.current) return;
      const wasMoving = dragRef.current.kind === 'move';
      dragRef.current = null;
      document.body.classList.remove('overlay-window-dragging');
      document.body.classList.remove('overlay-window-moving');
      // Final hole sync after last layout (class cleared so effect won't fire).
      if (wasMoving) {
        window.dispatchEvent(new Event(OVERLAY_WINDOW_MOVED));
      }
    };

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    return () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };
  }, [applyBounds, minWidth, minHeight, maxWidth, maxHeight]);

  const startMove = (event: React.MouseEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    onFocus();
    dragRef.current = {
      kind: 'move',
      startX: event.clientX,
      startY: event.clientY,
      startBounds: boundsRef.current,
    };
    document.body.classList.add('overlay-window-dragging');
    document.body.classList.add('overlay-window-moving');
  };

  const startResize = (edge: Edge) => (event: React.MouseEvent) => {
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
  };

  if (minimized) return null;

  const style: CSSProperties = {
    left: bounds.x,
    top: bounds.y,
    width: bounds.width,
    height: bounds.height,
    zIndex,
  };

  return (
    <div
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
      <header className="overlay-window-titlebar" onMouseDown={startMove}>
        <span className="overlay-window-title">{title}</span>
        <div className="overlay-window-controls">
          <button
            type="button"
            className="overlay-window-btn minimize"
            aria-label="Minimize"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={onMinimize}
          >
            ─
          </button>
          <button
            type="button"
            className="overlay-window-btn close"
            aria-label="Close"
            onMouseDown={(e) => e.stopPropagation()}
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
          onMouseDown={startResize(edge)}
          aria-hidden
        />
      ))}
    </div>
  );
}
