import { Dock, DockIcon, DockLabel, cn } from '@glint/ui';
import { getAppFallbackIcon } from '../lib/app-icons';
import type { AppPanelTab } from '../hooks/useOverlayWindows';

type Props = {
  tabs: AppPanelTab[];
  openKeys: Set<string>;
  focusedKey: string | null;
  onToggle: (tab: AppPanelTab) => void;
};

export function OverlayDock({ tabs, openKeys, focusedKey, onToggle }: Props) {
  return (
    <footer className="overlay-dock" aria-label="Apps">
      <Dock>
        {tabs.map((tab) => {
          const isOpen = openKeys.has(tab.key);
          const isActive = focusedKey === tab.key && isOpen;
          return (
            <DockIcon
              key={tab.key}
              role="button"
              tabIndex={0}
              aria-label={tab.title}
              aria-pressed={isOpen}
              className={cn(
                'text-[var(--text-muted)]',
                isOpen && 'text-[var(--text)] opacity-100',
                !isOpen && 'opacity-70',
                isActive &&
                  'bg-[rgba(125,211,252,0.12)] text-[var(--cyan)] ring-1 ring-[rgba(125,211,252,0.4)]',
              )}
              onClick={() => onToggle(tab)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  onToggle(tab);
                }
              }}
            >
              <DockLabel label={tab.title}>
                <span className="flex size-full items-center justify-center" aria-hidden>
                  {tab.iconUrl ? (
                    <img
                      src={tab.iconUrl}
                      alt=""
                      draggable={false}
                      className="size-[65%] object-contain"
                    />
                  ) : (
                    getAppFallbackIcon(tab.manifestId, tab.title)
                  )}
                </span>
              </DockLabel>
            </DockIcon>
          );
        })}
      </Dock>
    </footer>
  );
}
