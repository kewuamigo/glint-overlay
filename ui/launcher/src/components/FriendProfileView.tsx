import { motion } from 'framer-motion';
import type { MockFriend } from '../data/mockFriends';
import { mockSocialPosts } from '../data/mockFriends';

type Props = {
  friend: MockFriend;
  onBack: () => void;
};

function statusLabel(friend: MockFriend): string {
  if (friend.game) return `Playing ${friend.game}`;
  if (friend.status === 'online') return 'Online';
  if (friend.status === 'away') return 'Away';
  return 'Offline';
}

export function FriendProfileView({ friend, onBack }: Props) {
  const avg =
    friend.achievements.length === 0
      ? 0
      : Math.round(
          friend.achievements.reduce((s, a) => s + a.progress, 0) /
            friend.achievements.length,
        );
  const posts = mockSocialPosts.filter((p) => p.author === friend.name).slice(0, 2);

  return (
    <motion.div
      className="friend-profile"
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.28 }}
    >
      <section
        className="friend-profile-hero"
        style={{
          background: `linear-gradient(125deg, ${friend.avatarColor}55 0%, #07080c 55%, #050607 100%)`,
        }}
      >
        <button type="button" className="btn-ghost back-btn" onClick={onBack}>
          ← Back
        </button>

        <div className="friend-profile-hero-body">
          <div
            className="friend-profile-avatar"
            style={{ background: friend.avatarColor }}
          >
            {friend.name.slice(0, 2).toUpperCase()}
            <span className={`friend-status-dot ${friend.status}`} />
          </div>
          <div className="friend-profile-hero-copy">
            <span className={`pill${friend.status === 'online' || friend.game ? ' pill-live' : ''}`}>
              {statusLabel(friend)}
            </span>
            <h1 className="friend-profile-name">{friend.name}</h1>
            <p className="friend-profile-bio">{friend.bio}</p>
            <div className="friend-profile-actions">
              <button type="button" className="btn-primary" disabled title="Coming soon">
                Invite to party
              </button>
              <button type="button" className="btn-ghost" disabled title="Coming soon">
                Message
              </button>
            </div>
          </div>
        </div>
      </section>

      <div className="friend-profile-stats">
        <div className="friend-stat glass">
          <strong>{friend.level}</strong>
          <span>Level</span>
        </div>
        <div className="friend-stat glass">
          <strong>{friend.achievements.length}</strong>
          <span>Tracked trophies</span>
        </div>
        <div className="friend-stat glass">
          <strong>{avg}%</strong>
          <span>Avg progress</span>
        </div>
        <div className="friend-stat glass">
          <strong className={`friend-stat-status ${friend.status}`}>
            {friend.status}
          </strong>
          <span>Presence</span>
        </div>
      </div>

      <div className="friend-profile-grid">
        <section className="friend-profile-panel">
          <div className="home-section-head">
            <h2>Achievements</h2>
          </div>
          <div className="friend-trophy-list">
            {friend.achievements.map((a) => (
              <article
                key={a.title}
                className={`trophy-card glass${a.progress >= 100 ? ' on' : ' locked'}`}
              >
                <span className="achievements-dot" />
                <div>
                  <strong>{a.title}</strong>
                  <p>{a.game}</p>
                  <div className="progress-bar" style={{ marginTop: 8 }}>
                    <div className="progress-fill" style={{ width: `${a.progress}%` }} />
                  </div>
                  <em>{a.progress >= 100 ? 'Unlocked' : `${a.progress}%`}</em>
                </div>
              </article>
            ))}
          </div>
        </section>

        <section className="friend-profile-panel">
          <div className="home-section-head">
            <h2>Activity</h2>
          </div>
          {posts.length === 0 ? (
            <div className="glass friend-profile-empty">
              <p className="home-muted" style={{ margin: 0 }}>
                No recent posts from {friend.name}. When social goes live, their
                feed will show up here.
              </p>
            </div>
          ) : (
            <div className="friend-activity-list">
              {posts.map((post) => (
                <article key={post.id} className="social-card glass">
                  <div className="social-card-head">
                    <span
                      className="friend-avatar"
                      style={{ background: post.avatarColor }}
                    >
                      {post.author.slice(0, 2).toUpperCase()}
                    </span>
                    <div>
                      <strong>{post.author}</strong>
                      <em>{post.meta}</em>
                    </div>
                  </div>
                  <h3>{post.title}</h3>
                  <p>{post.body}</p>
                </article>
              ))}
            </div>
          )}

          {friend.game && (
            <div className="glass friend-now-playing">
              <span className="news-tag">Now playing</span>
              <strong>{friend.game}</strong>
              <p>Join when invites land — party flow comes next.</p>
            </div>
          )}
        </section>
      </div>
    </motion.div>
  );
}
