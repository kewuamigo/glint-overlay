import { motion } from 'framer-motion';
import { mockAchievements, mockNews } from '../data/mockFriends';
import type { ScannedGame } from '../launcher-bridge';

type Props = {
  featured: ScannedGame | null;
  launching: string | null;
  onLaunchFeatured: () => void;
};

export function RightPanel({ featured, launching, onLaunchFeatured }: Props) {
  const running = Boolean(featured?.running && featured.pid);
  const canAct = Boolean(featured);
  const label = launching
    ? running
      ? 'Attaching…'
      : 'Starting…'
    : running
      ? 'Quick attach'
      : 'Play';

  return (
    <aside className="right-panel">
      <motion.button
        type="button"
        className="launch-cta"
        disabled={!canAct || Boolean(launching)}
        onClick={onLaunchFeatured}
        whileHover={canAct ? { scale: 1.01 } : undefined}
        whileTap={canAct ? { scale: 0.98 } : undefined}
      >
        {label}
      </motion.button>

      <div className="widget-card glass">
        <div className="widget-title">Recent progress</div>
        <div className="widget-body-text">{mockAchievements.game}</div>
        <div className="progress-bar">
          <motion.div
            className="progress-fill"
            initial={{ width: 0 }}
            animate={{ width: `${mockAchievements.progress}%` }}
            transition={{ duration: 0.6, ease: 'easeOut' }}
          />
        </div>
        <div className="widget-caption">{mockAchievements.progress}% complete</div>
      </div>

      <div className="widget-card glass widget-grow">
        <div className="widget-title">Feed</div>
        {mockNews.map((item) => (
          <div key={item.title} className="news-item">
            <span className="news-tag">{item.tag}</span>
            {item.title}
          </div>
        ))}
      </div>
    </aside>
  );
}
