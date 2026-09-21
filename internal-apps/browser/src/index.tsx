import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import type { PluginAPI, PluginComponent } from '@glint/plugin-sdk';

type BrowserTab = { id: string; title: string; url: string };
type Bookmark = { id: string; title: string; url: string };
type BrowserSession = { tabs: BrowserTab[]; activeId: string };
type ExtensionEntry = { id: string; enabled: boolean };

const BROWSER_SESSION_KEY = 'glint.browser-session';
const BOOKMARKS_KEY = 'glint.bookmarks-v2';
const BOOKMARKS_LEGACY_KEY = 'glint.bookmarks';

/** Host methods without pluginId (navigate / session). */
function hostInvoke(method: string, args: unknown[] = []): Promise<unknown> {
  const bridge = (
    window as Window & {
      __goHost?: {
        invoke: (m: string, a: unknown[], p?: string) => Promise<unknown>;
      };
    }
  ).__goHost;
  if (!bridge) return Promise.resolve(undefined);
  return bridge.invoke(method, args);
}

function isNewTab(tab: BrowserTab): boolean {
  return !tab.url || tab.url === 'about:blank';
}

function newTab(url = ''): BrowserTab {
  return {
    id: crypto.randomUUID(),
    title: 'New Tab',
    url,
  };
}

function normalizeUrl(raw: string): string {
  const s = raw.trim();
  if (!s) return '';
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(s)) return s;
  if (s.includes(' ') || !s.includes('.')) {
    return `https://duckduckgo.com/?q=${encodeURIComponent(s)}`;
  }
  return `https://${s}`;
}

function bookmarkLabel(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '') || url;
  } catch {
    return url.slice(0, 32);
  }
}

function loadBookmarks(): Bookmark[] {
  try {
    const raw =
      localStorage.getItem(BOOKMARKS_KEY) ||
      localStorage.getItem(BOOKMARKS_LEGACY_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as Bookmark[];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((b) => b && typeof b.url === 'string' && b.url);
  } catch {
    return [];
  }
}

function saveBookmarks(next: Bookmark[]): void {
  try {
    localStorage.setItem(BOOKMARKS_KEY, JSON.stringify(next));
  } catch {
    /* ignore */
  }
}

function loadBrowserSession(): BrowserSession | null {
  try {
    const raw = localStorage.getItem(BROWSER_SESSION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as BrowserSession;
    if (!Array.isArray(parsed.tabs) || parsed.tabs.length === 0) return null;
    if (!parsed.tabs.some((t) => t.id === parsed.activeId)) return null;
    return parsed;
  } catch {
    return null;
  }
}

function saveBrowserSession(session: BrowserSession): void {
  try {
    localStorage.setItem(BROWSER_SESSION_KEY, JSON.stringify(session));
  } catch {
    /* ignore */
  }
}

function initialBrowserState(): BrowserSession {
  const saved = loadBrowserSession();
  if (saved) return saved;
  const tab = newTab();
  return { tabs: [tab], activeId: tab.id };
}

function NewTabPage({ onNavigate }: { onNavigate: (raw: string) => void }) {
  const [query, setQuery] = useState('');
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!query.trim()) return;
    onNavigate(query);
  };
  return (
    <div className="browser-new-tab">
      <h2 className="browser-new-tab-title">New Tab</h2>
      <p className="browser-new-tab-sub muted">
        Search or enter a URL to get started
      </p>
      <form className="browser-new-tab-form" onSubmit={submit}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search or enter URL"
          spellCheck={false}
          autoFocus
        />
        <button type="submit">Go</button>
      </form>
    </div>
  );
}

function BrowserPanel({ api }: { api: PluginAPI }) {
  const initial = useRef(initialBrowserState());
  const [tabs, setTabs] = useState<BrowserTab[]>(() => initial.current.tabs);
  const [activeId, setActiveId] = useState(initial.current.activeId);
  const activeTab = tabs.find((t) => t.id === activeId) ?? tabs[0]!;
  const [urlInput, setUrlInput] = useState(
    isNewTab(activeTab) ? '' : activeTab.url,
  );
  const [bookmarks, setBookmarks] = useState<Bookmark[]>(() => loadBookmarks());
  const [nav, setNav] = useState({ canGoBack: false, canGoForward: false });
  const [bookmarksOpen, setBookmarksOpen] = useState(false);
  const [extensionsOpen, setExtensionsOpen] = useState(false);
  const [extensions, setExtensions] = useState<ExtensionEntry[]>([]);
  const [extensionsRestartHint, setExtensionsRestartHint] = useState(false);
  const [cwsInput, setCwsInput] = useState('');
  const [cwsBusy, setCwsBusy] = useState(false);
  const [cwsError, setCwsError] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;

  const updateTabTitle = useCallback((tabId: string, title: string) => {
    setTabs((prev) =>
      prev.map((t) => (t.id === tabId ? { ...t, title } : t)),
    );
  }, []);

  const updateTabUrl = useCallback(
    (tabId: string, url: string) => {
      setTabs((prev) =>
        prev.map((t) => (t.id === tabId ? { ...t, url } : t)),
      );
      if (tabId === activeId) setUrlInput(url);
    },
    [activeId],
  );

  const reportContentRect = useCallback(() => {
    const el = contentRef.current;
    if (!el) {
      void api.browser.setContentRect(null);
      return;
    }
    const rect = el.getBoundingClientRect();
    void api.browser.setContentRect({
      x: rect.x,
      y: rect.y,
      width: rect.width,
      height: rect.height,
    });
  }, [api]);

  const navigate = useCallback(
    (raw: string, tabId = activeId) => {
      const url = normalizeUrl(raw);
      if (!url) return;
      setTabs((prev) =>
        prev.map((t) =>
          t.id === tabId ? { ...t, url, title: bookmarkLabel(url) } : t,
        ),
      );
      if (tabId === activeId) setUrlInput(url);
      if (tabId === activeId) {
        void hostInvoke('browser.navigate', [url]);
        void api.browser.focus();
      }
    },
    [activeId, api],
  );

  const switchToTab = useCallback(
    (id: string) => {
      if (id === activeId) return;
      setActiveId(id);
      const tab = tabs.find((t) => t.id === id);
      if (tab && !isNewTab(tab)) {
        void hostInvoke('browser.navigate', [tab.url]);
        void api.browser.focus();
      }
    },
    [activeId, tabs, api],
  );

  useEffect(() => {
    const tab = tabs.find((t) => t.id === activeId);
    setUrlInput(tab && isNewTab(tab) ? '' : (tab?.url ?? ''));
    if (tab && isNewTab(tab)) {
      void hostInvoke('browser.navigate', ['about:blank']);
      void api.browser.setContentRect(null);
      void api.browser.blur();
    } else {
      reportContentRect();
    }
  }, [activeId, tabs, reportContentRect, api]);

  useEffect(() => {
    const tab = tabs.find((t) => t.id === activeId);
    if (tab && !isNewTab(tab)) {
      void hostInvoke('browser.navigate', [tab.url]);
    }
    // Mount-only bootstrap.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const data = event.data as
        | {
            type: 'browser.navState';
            url?: string;
            title?: string;
            canGoBack?: boolean;
            canGoForward?: boolean;
          }
        | { type: 'browserSession'; open?: boolean }
        | { type: 'chrome'; overlayOpen?: boolean }
        | undefined;
      if (!data) return;
      if (data.type === 'browserSession') {
        return;
      }
      if (data.type === 'browser.navState') {
        setNav({
          canGoBack: Boolean(data.canGoBack),
          canGoForward: Boolean(data.canGoForward),
        });
        const tabId = activeIdRef.current;
        if (data.url && data.url !== 'about:blank') {
          updateTabUrl(tabId, data.url);
        }
        if (data.title) updateTabTitle(tabId, data.title);
      }
      if (data.type === 'chrome') {
        if (!data.overlayOpen) {
          void api.browser.setContentRect(null);
          return;
        }
        const restore = () => {
          const tab = tabsRef.current.find((t) => t.id === activeIdRef.current);
          if (tab && !isNewTab(tab)) {
            void api.browser.focus();
          }
          reportContentRect();
        };
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            restore();
            requestAnimationFrame(restore);
          });
        });
      }
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [api, reportContentRect, updateTabTitle, updateTabUrl]);

  useEffect(() => {
    reportContentRect();
    const ro = new ResizeObserver(() => reportContentRect());
    if (contentRef.current) ro.observe(contentRef.current);
    window.addEventListener('resize', reportContentRect);
    const onWindowMoved = () => reportContentRect();
    window.addEventListener('overlay-window-moved', onWindowMoved);
    return () => {
      void api.browser.setContentRect(null);
      void api.browser.blur();
      window.removeEventListener('resize', reportContentRect);
      window.removeEventListener('overlay-window-moved', onWindowMoved);
      ro.disconnect();
    };
  }, [reportContentRect, api]);

  useEffect(() => {
    saveBrowserSession({ tabs, activeId });
  }, [tabs, activeId]);

  const addTab = () => {
    const tab = newTab();
    setTabs((prev) => [...prev, tab]);
    setActiveId(tab.id);
  };

  const closeTab = useCallback((id: string) => {
    setTabs((prev) => {
      if (prev.length === 1) {
        // Last tab: reset to New Tab — activeId effect blanks content.
        const tab = newTab();
        setActiveId(tab.id);
        return [tab];
      }
      const next = prev.filter((t) => t.id !== id);
      if (activeIdRef.current === id) {
        setActiveId(next[0]!.id);
        // Content LoadURL / blank via activeId effect.
      }
      return next;
    });
  }, []);

  const activeBookmark = bookmarks.find((b) => b.url === activeTab.url);

  const toggleBookmark = () => {
    if (activeBookmark) {
      const next = bookmarks.filter((b) => b.id !== activeBookmark.id);
      setBookmarks(next);
      saveBookmarks(next);
      return;
    }
    const entry: Bookmark = {
      id: crypto.randomUUID(),
      title: activeTab.title.trim() || bookmarkLabel(activeTab.url),
      url: activeTab.url,
    };
    const next = [...bookmarks, entry];
    setBookmarks(next);
    saveBookmarks(next);
  };

  const runNav = (
    method: 'browser.goBack' | 'browser.goForward' | 'browser.reload',
  ) => {
    void api.browser.blur();
    void hostInvoke(method);
    requestAnimationFrame(() => {
      void api.browser.focus();
    });
  };

  const refreshExtensions = useCallback(async () => {
    const raw = await hostInvoke('browser.extensions.list');
    if (!Array.isArray(raw)) {
      setExtensions([]);
      return;
    }
    const next: ExtensionEntry[] = [];
    for (const item of raw) {
      if (!item || typeof item !== 'object') continue;
      const row = item as { id?: unknown; enabled?: unknown };
      if (typeof row.id !== 'string' || !row.id) continue;
      next.push({ id: row.id, enabled: row.enabled !== false });
    }
    setExtensions(next);
  }, []);

  const toggleExtensionsPanel = () => {
    setExtensionsOpen((open) => {
      const next = !open;
      if (next) {
        setBookmarksOpen(false);
        setExtensionsRestartHint(false);
        void refreshExtensions();
      }
      return next;
    });
  };

  const setExtensionEnabled = async (id: string, enabled: boolean) => {
    try {
      const raw = await hostInvoke('browser.extensions.setEnabled', [
        id,
        enabled,
      ]);
      const restartRequired =
        raw &&
        typeof raw === 'object' &&
        (raw as { restartRequired?: unknown }).restartRequired === true;
      if (restartRequired) setExtensionsRestartHint(true);
      setExtensions((prev) =>
        prev.map((e) => (e.id === id ? { ...e, enabled } : e)),
      );
    } catch {
      void refreshExtensions();
    }
  };

  const openExtensionOptions = (id: string) => {
    void hostInvoke('browser.extensions.openOptions', [id]);
  };

  const openExtensionPopup = (id: string) => {
    void hostInvoke('browser.extensions.openPopup', [id]);
  };

  const installFromStore = async () => {
    const raw = cwsInput.trim();
    if (!raw || cwsBusy) return;
    setCwsBusy(true);
    setCwsError(null);
    try {
      const result = await hostInvoke('browser.extensions.installFromStore', [
        raw,
      ]);
      const restartRequired =
        result &&
        typeof result === 'object' &&
        (result as { restartRequired?: unknown }).restartRequired === true;
      if (restartRequired) setExtensionsRestartHint(true);
      setCwsInput('');
      void refreshExtensions();
    } catch (err) {
      const msg =
        err instanceof Error
          ? err.message
          : typeof err === 'string'
            ? err
            : 'Install failed';
      setCwsError(msg);
    } finally {
      setCwsBusy(false);
    }
  };

  const showHole = !isNewTab(activeTab);

  return (
    <section
      aria-label="In-game Browser"
      className="browser-panel browser-panel-embedded"
    >
      <div className="browser-toolbar">
        <div className="browser-toolbar-nav">
          <button
            type="button"
            title="Back"
            disabled={!nav.canGoBack}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              if (nav.canGoBack) runNav('browser.goBack');
            }}
          >
            ←
          </button>
          <button
            type="button"
            title="Forward"
            disabled={!nav.canGoForward}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              if (nav.canGoForward) runNav('browser.goForward');
            }}
          >
            →
          </button>
          <button
            type="button"
            title="Reload"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => runNav('browser.reload')}
          >
            ↻
          </button>
        </div>

        <form
          className="browser-url-row"
          onSubmit={(e) => {
            e.preventDefault();
            navigate(urlInput);
          }}
        >
          <input
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            onFocus={() => {
              // Explicit chrome steal — host blur grants chrome SetFocus
              // (hit-test alone only flips on cursor move).
              void api.browser.blur();
            }}
            placeholder="Search or enter URL"
            spellCheck={false}
          />
          <button type="submit">Go</button>
        </form>

        <div className="browser-toolbar-actions">
          <button
            type="button"
            className={activeBookmark ? 'browser-star active' : 'browser-star'}
            title={activeBookmark ? 'Remove bookmark' : 'Bookmark this page'}
            disabled={isNewTab(activeTab)}
            onClick={toggleBookmark}
          >
            {activeBookmark ? '★' : '☆'}
          </button>
          <button
            type="button"
            className={
              bookmarksOpen
                ? 'browser-bookmarks-toggle active'
                : 'browser-bookmarks-toggle'
            }
            title="Bookmarks"
            onClick={() => {
              setBookmarksOpen((open) => {
                const next = !open;
                if (next) setExtensionsOpen(false);
                return next;
              });
            }}
          >
            ☰
          </button>
          <button
            type="button"
            className={
              extensionsOpen
                ? 'browser-extensions-toggle active'
                : 'browser-extensions-toggle'
            }
            title="Extension sideload status"
            onClick={toggleExtensionsPanel}
          >
            Ext
          </button>
        </div>
      </div>

      <div className="browser-tab-row">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            type="button"
            className={tab.id === activeId ? 'browser-tab active' : 'browser-tab'}
            onClick={() => switchToTab(tab.id)}
          >
            <span className="browser-tab-title">
              {isNewTab(tab) ? 'New Tab' : tab.title.slice(0, 28)}
            </span>
            <span
              className="browser-tab-close"
              onClick={(e) => {
                e.stopPropagation();
                closeTab(tab.id);
              }}
              aria-label="Close tab"
            >
              ×
            </span>
          </button>
        ))}
        <button
          type="button"
          className="browser-tab-add"
          title="New tab"
          onClick={(e) => {
            e.stopPropagation();
            addTab();
          }}
        >
          +
        </button>
      </div>

      {bookmarks.length > 0 && (
        <div className="browser-bookmarks-bar">
          {bookmarks.map((bookmark) => (
            <div key={bookmark.id} className="browser-bookmark-chip">
              <button
                type="button"
                className="browser-bookmark-chip-label"
                title={bookmark.url}
                onClick={() => navigate(bookmark.url)}
              >
                {bookmark.title.slice(0, 32)}
              </button>
              <button
                type="button"
                className="browser-bookmark-chip-remove"
                title="Remove"
                onClick={() => {
                  const next = bookmarks.filter((b) => b.id !== bookmark.id);
                  setBookmarks(next);
                  saveBookmarks(next);
                }}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}

      {bookmarksOpen && (
        <div className="browser-bookmarks-panel">
          {bookmarks.length === 0 ? (
            <p className="muted">No bookmarks yet</p>
          ) : (
            bookmarks.map((b) => (
              <button
                key={b.id}
                type="button"
                className="browser-bookmark-row"
                onClick={() => {
                  setBookmarksOpen(false);
                  navigate(b.url);
                }}
              >
                {b.title}
              </button>
            ))
          )}
        </div>
      )}

      {extensionsOpen && (
        <div className="browser-extensions-panel" aria-label="Extension status">
          <div className="browser-extensions-panel-header">
            Unpacked extensions
          </div>
          <p className="browser-extensions-note muted">
            Paste a Chrome Web Store URL or extension id to download/unpack into
            %APPDATA%\Glint\BrowserExtensions. In-page Add to Chrome on Alloy OSR
            is not supported. Enablement needs a CEF helper restart after install.
            Options/Popup open desktop satellites — not a Chrome toolbar.
          </p>
          <form
            className="browser-extensions-install"
            onSubmit={(e) => {
              e.preventDefault();
              void installFromStore();
            }}
          >
            <input
              value={cwsInput}
              onChange={(e) => setCwsInput(e.target.value)}
              placeholder="CWS URL or extension id"
              spellCheck={false}
              disabled={cwsBusy}
            />
            <button type="submit" disabled={cwsBusy || !cwsInput.trim()}>
              {cwsBusy ? 'Installing…' : 'Install'}
            </button>
          </form>
          {cwsError && (
            <p className="browser-extensions-error" role="alert">
              {cwsError}
            </p>
          )}
          {extensionsRestartHint && (
            <p className="browser-extensions-restart" role="status">
              Restart the overlay browser for changes to take effect.
            </p>
          )}
          {extensions.length === 0 ? (
            <p className="muted browser-extensions-empty">
              No unpacked extensions found
            </p>
          ) : (
            <ul className="browser-extensions-list">
              {extensions.map((ext) => (
                <li key={ext.id} className="browser-extensions-row">
                  <span className="browser-extensions-id" title={ext.id}>
                    {ext.id}
                  </span>
                  <div className="browser-extensions-actions">
                    <button
                      type="button"
                      className="browser-extensions-action"
                      title="Open extension options (desktop satellite)"
                      disabled={!ext.enabled}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => openExtensionOptions(ext.id)}
                    >
                      Options
                    </button>
                    <button
                      type="button"
                      className="browser-extensions-action"
                      title="Open extension popup (best-effort satellite)"
                      disabled={!ext.enabled}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => openExtensionPopup(ext.id)}
                    >
                      Popup
                    </button>
                    <label className="browser-extensions-enable">
                      <input
                        type="checkbox"
                        checked={ext.enabled}
                        onChange={(e) => {
                          void setExtensionEnabled(ext.id, e.target.checked);
                        }}
                      />
                      Enabled
                    </label>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="browser-content-host">
        {isNewTab(activeTab) && (
          <NewTabPage onNavigate={(raw) => navigate(raw)} />
        )}
        <div
          ref={contentRef}
          className={
            showHole
              ? 'browser-cef-hole browser-cef-hole-active'
              : 'browser-cef-hole browser-cef-hole-hidden'
          }
          aria-hidden
        />
      </div>
    </section>
  );
}

export const Plugin: PluginComponent = ({ api }) => (
  <div className="browser-app-root browser-panel-host">
    <BrowserPanel api={api} />
  </div>
);

export default Plugin;
