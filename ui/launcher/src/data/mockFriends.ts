export type MockFriend = {
  name: string;
  status: 'online' | 'offline' | 'away';
  game?: string;
  avatarColor: string;
  bio: string;
  level: number;
  achievements: { title: string; game: string; progress: number }[];
};

export type MockSocialPost = {
  id: string;
  author: string;
  avatarColor: string;
  kind: 'image' | 'status';
  title: string;
  body: string;
  meta: string;
};

export const mockFriends: MockFriend[] = [
  {
    name: 'ApexPredator',
    status: 'online',
    game: 'Valorant',
    avatarColor: '#ff6b2b',
    bio: 'Competitive FPS main. Always down for ranked.',
    level: 42,
    achievements: [
      { title: 'Headshot Machine', game: 'Valorant', progress: 100 },
      { title: 'Radiant Dreams', game: 'Valorant', progress: 67 },
    ],
  },
  {
    name: 'ShadowStriker',
    status: 'online',
    game: 'Elden Ring',
    avatarColor: '#4a90d9',
    bio: 'Souls veteran. Bosses fear me.',
    level: 58,
    achievements: [
      { title: 'Elden Lord', game: 'Elden Ring', progress: 100 },
      { title: 'All Remembrances', game: 'Elden Ring', progress: 82 },
    ],
  },
  {
    name: 'NeonDrift',
    status: 'away',
    avatarColor: '#9b59b6',
    bio: 'Racing games & synthwave.',
    level: 31,
    achievements: [{ title: 'Speed Demon', game: 'Forza', progress: 45 }],
  },
  {
    name: 'CyberWolf',
    status: 'offline',
    avatarColor: '#2ecc71',
    bio: 'Night owl gamer.',
    level: 27,
    achievements: [{ title: 'Night City Legend', game: 'Cyberpunk 2077', progress: 94 }],
  },
  {
    name: 'PixelRogue',
    status: 'online',
    game: 'Minecraft',
    avatarColor: '#e74c3c',
    bio: 'Builder & redstone nerd.',
    level: 19,
    achievements: [{ title: 'Master Builder', game: 'Minecraft', progress: 72 }],
  },
  {
    name: 'StormBreaker',
    status: 'offline',
    avatarColor: '#f39c12',
    bio: 'Co-op enthusiast.',
    level: 35,
    achievements: [{ title: 'Team Player', game: 'Overwatch 2', progress: 55 }],
  },
];

export const mockNews = [
  { title: 'Patch 2.1 — balance updates', tag: 'Updates' },
  { title: 'Weekend double XP event', tag: 'Events' },
  { title: 'New overlay apps in Store', tag: 'News' },
];

export const mockAchievements = {
  game: 'Featured Game',
  progress: 94,
};

export const mockSocialPosts: MockSocialPost[] = [
  {
    id: '1',
    author: 'ApexPredator',
    avatarColor: '#ff6b2b',
    kind: 'image',
    title: 'LEVEL UP!',
    body: 'Hit a new peak after a long grind. Overlay stats finally make sense.',
    meta: '2h ago · 48 likes',
  },
  {
    id: '2',
    author: 'ShadowStriker',
    avatarColor: '#4a90d9',
    kind: 'status',
    title: 'Boss down',
    body: 'Third try was the charm. Anyone else stuck on the last phase?',
    meta: '5h ago · 12 comments',
  },
  {
    id: '3',
    author: 'PixelRogue',
    avatarColor: '#e74c3c',
    kind: 'status',
    title: 'Looking for co-op',
    body: 'Building a survival world tonight. Bring redstone opinions.',
    meta: 'Yesterday · 6 likes',
  },
  {
    id: '4',
    author: 'NeonDrift',
    avatarColor: '#9b59b6',
    kind: 'image',
    title: 'Night circuit',
    body: 'Clean lap under the neon. Screenshot from the last session.',
    meta: 'Yesterday · 31 likes',
  },
];
