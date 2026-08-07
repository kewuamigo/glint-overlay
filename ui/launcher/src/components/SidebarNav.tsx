import { motion } from 'framer-motion';
import type { LauncherTab } from '../launcher-bridge';

const NAV: { id: LauncherTab; label: string }[] = [
  { id: 'home', label: 'Home' },
  { id: 'library', label: 'Library' },
  { id: 'store', label: 'Store' },
  { id: 'achievements', label: 'Achievements' },
  { id: 'settings', label: 'Settings' },
];

function NavIcon({ tab }: { tab: LauncherTab }) {
  const props = {
    width: 20,
    height: 20,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 1.75,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    'aria-hidden': true as const,
  };
  switch (tab) {
    case 'home':
      return (
        <svg {...props}>
          <rect x="3" y="3" width="7" height="7" rx="1.5" />
          <rect x="14" y="3" width="7" height="7" rx="1.5" />
          <rect x="3" y="14" width="7" height="7" rx="1.5" />
          <rect x="14" y="14" width="7" height="7" rx="1.5" />
        </svg>
      );
    case 'library':
      return (
        <svg {...props}>
          <path d="M4 6h12M4 12h16M4 18h10" />
        </svg>
      );
    case 'store':
      return (
        <svg {...props}>
          <rect x="3" y="7" width="18" height="13" rx="2" />
          <path d="M8 7V5a4 4 0 0 1 8 0v2" />
        </svg>
      );
    case 'achievements':
      return (
        <svg {...props}>
          <path d="M8 21h8" />
          <path d="M12 17v4" />
          <path d="M7 4h10v5a5 5 0 0 1-10 0V4Z" />
          <path d="M5 6H3a2 2 0 0 0 2 4" />
          <path d="M19 6h2a2 2 0 0 1-2 4" />
        </svg>
      );
    default:
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="3" />
          <path d="M12 2v2M12 20v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M2 12h2M20 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4" />
        </svg>
      );
  }
}

type Props = {
  tab: LauncherTab;
  onTab: (tab: LauncherTab) => void;
};

export function SidebarNav({ tab, onTab }: Props) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand" title="Glint">
        <span className="sidebar-brand-mark">G</span>
      </div>
      <nav className="sidebar-nav" aria-label="Main">
        {NAV.map((item) => {
          const active = tab === item.id;
          return (
            <button
              key={item.id}
              type="button"
              className={`nav-item${active ? ' active' : ''}`}
              aria-label={item.label}
              aria-current={active ? 'page' : undefined}
              title={item.label}
              onClick={() => onTab(item.id)}
            >
              {active && (
                <motion.span
                  layoutId="nav-pill"
                  className="nav-pill"
                  transition={{ type: 'spring', stiffness: 420, damping: 32 }}
                />
              )}
              <span className="nav-item-inner">
                <NavIcon tab={item.id} />
              </span>
            </button>
          );
        })}
      </nav>
    </aside>
  );
}
