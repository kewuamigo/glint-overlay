import { useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import type { LauncherTab, ScannedGame } from './launcher-bridge';
import { SidebarNav } from './components/SidebarNav';
import { TopBar } from './components/TopBar';
import { HomeView } from './components/HomeView';
import { LibraryView } from './components/LibraryView';
import { StoreView } from './components/StoreView';
import { SettingsView } from './components/SettingsView';
import { AchievementsView } from './components/AchievementsView';
import { FriendProfileView } from './components/FriendProfileView';
import { useLauncher } from './hooks/useLauncher';
import { useFavorites } from './hooks/useFavorites';
import type { MockFriend } from './data/mockFriends';

const pageMotion = {
  initial: { opacity: 0, y: 8 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -6 },
  transition: { duration: 0.22, ease: [0.22, 1, 0.36, 1] as const },
};

export function LauncherApp() {
  const [tab, setTab] = useState<LauncherTab>('home');
  const [selected, setSelected] = useState<ScannedGame | null>(null);
  const [selectedFriend, setSelectedFriend] = useState<MockFriend | null>(null);
  const launcher = useLauncher();
  const favorites = useFavorites();

  const featured = selected ?? launcher.featured;
  const favoriteGames = favorites.favoriteIds
    .map((id) => launcher.games.find((g) => g.id === id))
    .filter((g): g is ScannedGame => Boolean(g));
  const contentKey = selectedFriend
    ? `friend-${selectedFriend.name}`
    : launcher.loading && tab !== 'settings'
      ? 'loading'
      : tab;

  const goTab = (t: LauncherTab) => {
    setSelectedFriend(null);
    setTab(t);
  };

  const attachGame = (game: ScannedGame) => {
    if (game.pid) void launcher.launchOverlay(game.pid, game);
  };

  return (
    <div className={`launcher${tab === 'home' && !selectedFriend ? ' launcher-home' : ''}`}>
      <SidebarNav tab={tab} onTab={goTab} />
      <div className="launcher-main">
        <TopBar
          profile={launcher.profile}
          favorites={favoriteGames}
          covers={launcher.covers}
          onSelectFavorite={(game) => {
            setSelected(game);
            setSelectedFriend(null);
            setTab('home');
          }}
        />
        <AnimatePresence>
          {launcher.error && (
            <motion.div
              key="launcher-error"
              className="error-banner"
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.16, ease: 'easeOut' }}
            >
              {launcher.error}
            </motion.div>
          )}
        </AnimatePresence>
        <div className="launcher-content">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div key={contentKey} className="page-shell" {...pageMotion}>
              {selectedFriend ? (
                <FriendProfileView
                  friend={selectedFriend}
                  onBack={() => setSelectedFriend(null)}
                />
              ) : launcher.loading && tab !== 'settings' ? (
                <div className="empty-state">Loading…</div>
              ) : tab === 'home' ? (
                <HomeView
                  games={launcher.games}
                  featured={featured}
                  launching={launcher.launching}
                  covers={launcher.covers}
                  catalog={launcher.catalog}
                  isFavorite={favorites.isFavorite}
                  onToggleFavorite={favorites.toggleFavorite}
                  onAttach={attachGame}
                  onPlay={(game) => void launcher.launchGame(game)}
                  onSelectGame={setSelected}
                  onOpenLibrary={() => goTab('library')}
                  onOpenStore={() => goTab('store')}
                  onOpenAchievements={() => goTab('achievements')}
                  onSelectFriend={(friend) => {
                    setSelectedFriend(friend);
                    setTab('home');
                  }}
                />
              ) : tab === 'library' ? (
                <LibraryView
                  games={launcher.games}
                  featured={featured}
                  launching={launcher.launching}
                  covers={launcher.covers}
                  onAttach={attachGame}
                  onPlay={(game) => void launcher.launchGame(game)}
                  onSelectGame={setSelected}
                  onAddGame={(name, exe) => void launcher.addGame(name, exe)}
                  onReset={() => {
                    setSelected(null);
                    void launcher.resetLibrary();
                  }}
                />
              ) : tab === 'store' ? (
                <StoreView
                  catalog={launcher.catalog}
                  installed={launcher.installed}
                  installing={launcher.installing}
                  onInstall={(id) => void launcher.installApp(id)}
                />
              ) : tab === 'achievements' ? (
                <AchievementsView
                  games={launcher.games}
                  covers={launcher.covers}
                />
              ) : (
                <SettingsView
                  onRefresh={() => void launcher.refreshAll()}
                  onSteamKeySaved={(configured) =>
                    launcher.setSteamKeyConfigured(configured)
                  }
                  onCoversKeySaved={() => launcher.refreshCovers()}
                />
              )}
            </motion.div>
          </AnimatePresence>
        </div>
      </div>
    </div>
  );
}
