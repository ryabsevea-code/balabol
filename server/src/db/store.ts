import fs from 'fs';
import path from 'path';
import bcrypt from 'bcryptjs';

export interface User {
  id: string;
  username: string;
  email: string;
  passwordHash: string;
  avatar: string;
  status: 'online' | 'idle' | 'dnd' | 'offline';
  customStatus?: string;
  createdAt: string;
}

export interface Channel {
  id: string;
  serverId: string | null; // null for DM channels
  name: string;
  type: 'text' | 'voice';
  description?: string;
}

export interface Server {
  id: string;
  name: string;
  icon?: string;
  ownerId: string;
  channelIds: string[];
}

export interface Message {
  id: string;
  channelId: string;
  authorId: string;
  content: string;
  attachments?: string[];
  createdAt: string;
}

export interface DMConversation {
  id: string;
  participantIds: [string, string];
  updatedAt: string;
}

export interface DMMessage {
  id: string;
  dmId: string;
  authorId: string;
  content: string;
  createdAt: string;
}

interface DatabaseSchema {
  users: User[];
  servers: Server[];
  channels: Channel[];
  messages: Message[];
  dms: DMConversation[];
  dmMessages: DMMessage[];
}

const DB_PATH = path.join(__dirname, '../../data/database.json');

class JsonDatabase {
  private data: DatabaseSchema = {
    users: [],
    servers: [],
    channels: [],
    messages: [],
    dms: [],
    dmMessages: [],
  };

  constructor() {
    this.init();
  }

  private init() {
    const dir = path.dirname(DB_PATH);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }

    if (fs.existsSync(DB_PATH)) {
      try {
        const raw = fs.readFileSync(DB_PATH, 'utf-8');
        this.data = JSON.parse(raw);
      } catch (err) {
        console.error('Error reading database file, re-initializing default state', err);
        this.seedDefaults();
      }
    } else {
      this.seedDefaults();
    }
  }

  private seedDefaults() {
    const defaultPasswordHash = bcrypt.hashSync('123456', 8);

    // Initial default demo users
    const defaultUsers: User[] = [
      {
        id: 'u-alex',
        username: 'Alex_Gamer',
        email: 'alex@balabol.local',
        passwordHash: defaultPasswordHash,
        avatar: 'https://api.dicebear.com/7.x/bottts/svg?seed=Alex',
        status: 'online',
        customStatus: 'Играет в Balabol',
        createdAt: new Date().toISOString(),
      },
      {
        id: 'u-dmitry',
        username: 'Dmitry_PRO',
        email: 'dmitry@balabol.local',
        passwordHash: defaultPasswordHash,
        avatar: 'https://api.dicebear.com/7.x/bottts/svg?seed=Dmitry',
        status: 'online',
        customStatus: 'В голосовом канале',
        createdAt: new Date().toISOString(),
      },
      {
        id: 'u-elena',
        username: 'Elena_Streamer',
        email: 'elena@balabol.local',
        passwordHash: defaultPasswordHash,
        avatar: 'https://api.dicebear.com/7.x/bottts/svg?seed=Elena',
        status: 'idle',
        customStatus: 'Отошла за чаем',
        createdAt: new Date().toISOString(),
      },
    ];

    const defaultServerId = 's-main';
    const defaultChannels: Channel[] = [
      { id: 'c-general', serverId: defaultServerId, name: 'общий-чат', type: 'text', description: 'Главный текстовый канал для общения' },
      { id: 'c-gaming', serverId: defaultServerId, name: 'игры', type: 'text', description: 'Обсуждение игр, стратегий и пати' },
      { id: 'c-memes', serverId: defaultServerId, name: 'мемы-и-флуд', type: 'text', description: 'Картинки, приколы и свободное общение' },
      { id: 'v-lounge', serverId: defaultServerId, name: 'Гостиная', type: 'voice', description: 'Основной голосовой канал' },
      { id: 'v-gaming-1', serverId: defaultServerId, name: 'Катка 1', type: 'voice', description: 'Тактический голосовой канал' },
      { id: 'v-chill', serverId: defaultServerId, name: 'Чиллаут', type: 'voice', description: 'Голос для спокойных бесед' },
    ];

    const defaultServer: Server = {
      id: defaultServerId,
      name: 'Сервер Балаболов',
      icon: 'https://api.dicebear.com/7.x/identicon/svg?seed=Balabol',
      ownerId: 'u-alex',
      channelIds: defaultChannels.map(c => c.id),
    };

    const initialMessages: Message[] = [
      {
        id: 'm-1',
        channelId: 'c-general',
        authorId: 'u-alex',
        content: 'Добро пожаловать в Balabol! Это наш собственный независимый голосовой и текстовый сервер.',
        createdAt: new Date(Date.now() - 3600000).toISOString(),
      },
      {
        id: 'm-2',
        channelId: 'c-general',
        authorId: 'u-dmitry',
        content: 'Привет! Качество звука в WebRTC отличное, и теперь можно регулировать громкость каждого тиммейта прямо тут!',
        createdAt: new Date(Date.now() - 1800000).toISOString(),
      },
    ];

    this.data = {
      users: defaultUsers,
      servers: [defaultServer],
      channels: defaultChannels,
      messages: initialMessages,
      dms: [],
      dmMessages: [],
    };

    this.save();
  }

  private save() {
    try {
      fs.writeFileSync(DB_PATH, JSON.stringify(this.data, null, 2), 'utf-8');
    } catch (err) {
      console.error('Failed to persist database:', err);
    }
  }

  // Users
  getUsers(): User[] {
    return this.data.users;
  }

  getUserById(id: string): User | undefined {
    return this.data.users.find(u => u.id === id);
  }

  getUserByEmail(email: string): User | undefined {
    return this.data.users.find(u => u.email.toLowerCase() === email.toLowerCase());
  }

  getUserByUsername(username: string): User | undefined {
    return this.data.users.find(u => u.username.toLowerCase() === username.toLowerCase());
  }

  createUser(user: User): User {
    this.data.users.push(user);
    this.save();
    return user;
  }

  updateUserStatus(id: string, status: User['status'], customStatus?: string) {
    const user = this.getUserById(id);
    if (user) {
      user.status = status;
      if (customStatus !== undefined) user.customStatus = customStatus;
      this.save();
    }
  }

  // Servers
  getServers(): Server[] {
    return this.data.servers;
  }

  getServerById(id: string): Server | undefined {
    return this.data.servers.find(s => s.id === id);
  }

  createServer(server: Server): Server {
    this.data.servers.push(server);
    this.save();
    return server;
  }

  // Channels
  getChannelsByServerId(serverId: string): Channel[] {
    return this.data.channels.filter(c => c.serverId === serverId);
  }

  getChannelById(id: string): Channel | undefined {
    return this.data.channels.find(c => c.id === id);
  }

  createChannel(channel: Channel): Channel {
    this.data.channels.push(channel);
    const server = this.getServerById(channel.serverId || '');
    if (server) {
      server.channelIds.push(channel.id);
    }
    this.save();
    return channel;
  }

  // Messages
  getMessagesByChannelId(channelId: string, limit = 100): Message[] {
    return this.data.messages
      .filter(m => m.channelId === channelId)
      .slice(-limit);
  }

  createMessage(msg: Message): Message {
    this.data.messages.push(msg);
    this.save();
    return msg;
  }

  // DMs
  getOrCreateDM(userA: string, userB: string): DMConversation {
    let dm = this.data.dms.find(
      d => (d.participantIds[0] === userA && d.participantIds[1] === userB) ||
           (d.participantIds[0] === userB && d.participantIds[1] === userA)
    );
    if (!dm) {
      dm = {
        id: `dm-${Date.now()}-${Math.random().toString(36).substring(2, 7)}`,
        participantIds: [userA, userB],
        updatedAt: new Date().toISOString(),
      };
      this.data.dms.push(dm);
      this.save();
    }
    return dm;
  }

  getUserDMs(userId: string): DMConversation[] {
    return this.data.dms.filter(d => d.participantIds.includes(userId));
  }

  getDMMessages(dmId: string, limit = 100): DMMessage[] {
    return this.data.dmMessages
      .filter(m => m.dmId === dmId)
      .slice(-limit);
  }

  createDMMessage(msg: DMMessage): DMMessage {
    this.data.dmMessages.push(msg);
    const dm = this.data.dms.find(d => d.id === msg.dmId);
    if (dm) {
      dm.updatedAt = msg.createdAt;
    }
    this.save();
    return msg;
  }
}

export const db = new JsonDatabase();
