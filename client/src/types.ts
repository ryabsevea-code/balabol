export interface User {
  id: string;
  username: string;
  email: string;
  avatar: string;
  status: 'online' | 'idle' | 'dnd' | 'offline';
  customStatus?: string;
  createdAt: string;
}

export interface Channel {
  id: string;
  serverId: string | null;
  name: string;
  type: 'text' | 'voice';
  description?: string;
}

export interface Server {
  id: string;
  name: string;
  icon?: string;
  ownerId: string;
  channels: Channel[];
}

export interface Message {
  id: string;
  channelId: string;
  authorId: string;
  content: string;
  attachments?: string[];
  createdAt: string;
  author: {
    id: string;
    username: string;
    avatar: string;
    status: User['status'];
  };
}

export interface VoiceParticipant {
  userId: string;
  socketId: string;
  username: string;
  avatar: string;
  isMuted: boolean;
  isDeafened: boolean;
  isSpeaking: boolean;
}

export interface DMConversation {
  id: string;
  participantIds: [string, string];
  updatedAt: string;
  otherUser: User | null;
  lastMessage?: {
    id: string;
    content: string;
    createdAt: string;
  } | null;
}
