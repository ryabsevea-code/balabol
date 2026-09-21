import { Server, Socket } from 'socket.io';
import { db } from '../db/store';

export interface VoiceParticipant {
  userId: string;
  socketId: string;
  username: string;
  avatar: string;
  isMuted: boolean;
  isDeafened: boolean;
  isSpeaking: boolean;
}

// Global state: channelId -> Map<userId, VoiceParticipant>
const voiceRooms = new Map<string, Map<string, VoiceParticipant>>();

// socketId -> { userId, channelId }
const socketVoiceMap = new Map<string, { userId: string; channelId: string }>();

// socketId -> userId
const socketUserMap = new Map<string, string>();

export function getGlobalVoiceState() {
  const result: Record<string, VoiceParticipant[]> = {};
  voiceRooms.forEach((participants, channelId) => {
    result[channelId] = Array.from(participants.values());
  });
  return result;
}

export function registerSocketHandlers(io: Server) {
  io.on('connection', (socket: Socket) => {
    // 1. Identify connected user
    socket.on('auth:identify', ({ userId }: { userId: string }) => {
      socketUserMap.set(socket.id, userId);
      db.updateUserStatus(userId, 'online');
      io.emit('presence:update', { userId, status: 'online' });
      // Send current global voice channels state
      socket.emit('voice:global-state', getGlobalVoiceState());
    });

    // 2. Channel Chat
    socket.on('chat:join-channel', ({ channelId }: { channelId: string }) => {
      socket.join(`channel:${channelId}`);
    });

    socket.on('chat:leave-channel', ({ channelId }: { channelId: string }) => {
      socket.leave(`channel:${channelId}`);
    });

    socket.on('chat:send-message', ({ channelId, content, authorId, attachments }: {
      channelId: string;
      content: string;
      authorId: string;
      attachments?: string[];
    }) => {
      if (!content || !content.trim()) return;

      const author = db.getUserById(authorId);
      const newMsg = db.createMessage({
        id: `m-${Date.now()}-${Math.random().toString(36).substring(2, 6)}`,
        channelId,
        authorId,
        content: content.trim(),
        attachments,
        createdAt: new Date().toISOString(),
      });

      const enrichedMsg = {
        ...newMsg,
        author: author ? {
          id: author.id,
          username: author.username,
          avatar: author.avatar,
          status: author.status,
        } : {
          id: authorId,
          username: 'Неизвестный',
          avatar: '',
          status: 'offline',
        },
      };

      io.to(`channel:${channelId}`).emit('chat:new-message', enrichedMsg);
    });

    socket.on('chat:typing', ({ channelId, username, isTyping }: {
      channelId: string;
      username: string;
      isTyping: boolean;
    }) => {
      socket.to(`channel:${channelId}`).emit('chat:user-typing', {
        channelId,
        username,
        isTyping,
      });
    });

    // 3. Direct Messages
    socket.on('dm:join', ({ dmId }: { dmId: string }) => {
      socket.join(`dm:${dmId}`);
    });

    socket.on('dm:send-message', ({ dmId, content, authorId }: {
      dmId: string;
      content: string;
      authorId: string;
    }) => {
      if (!content || !content.trim()) return;

      const author = db.getUserById(authorId);
      const newMsg = db.createDMMessage({
        id: `dm-m-${Date.now()}`,
        dmId,
        authorId,
        content: content.trim(),
        createdAt: new Date().toISOString(),
      });

      const enrichedMsg = {
        ...newMsg,
        author: author ? {
          id: author.id,
          username: author.username,
          avatar: author.avatar,
          status: author.status,
        } : {
          id: authorId,
          username: 'Неизвестный',
          avatar: '',
          status: 'offline',
        },
      };

      io.to(`dm:${dmId}`).emit('dm:new-message', enrichedMsg);
    });

    // 4. WebRTC Voice Channels
    socket.on('voice:join', ({ channelId, userId }: { channelId: string; userId: string }) => {
      // Leave any existing voice room first
      handleLeaveVoice(socket, io);

      const user = db.getUserById(userId);
      if (!user) return;

      if (!voiceRooms.has(channelId)) {
        voiceRooms.set(channelId, new Map());
      }

      const room = voiceRooms.get(channelId)!;
      const participant: VoiceParticipant = {
        userId: user.id,
        socketId: socket.id,
        username: user.username,
        avatar: user.avatar,
        isMuted: false,
        isDeafened: false,
        isSpeaking: false,
      };

      room.set(userId, participant);
      socketVoiceMap.set(socket.id, { userId, channelId });
      socket.join(`voice:${channelId}`);

      // Send existing participants in this room to the newly joined user
      const existingInRoom = Array.from(room.values()).filter(p => p.socketId !== socket.id);
      socket.emit('voice:users-in-room', {
        channelId,
        users: existingInRoom,
      });

      // Notify other members of this room that a new peer joined
      socket.to(`voice:${channelId}`).emit('voice:user-joined', {
        channelId,
        participant,
      });

      // Update sidebar channels list for everyone
      io.emit('voice:global-state', getGlobalVoiceState());
    });

    socket.on('voice:leave', () => {
      handleLeaveVoice(socket, io);
    });

    // WebRTC Signaling relay
    socket.on('voice:signal', ({ toSocketId, signalData }: {
      toSocketId: string;
      signalData: any;
    }) => {
      const voiceInfo = socketVoiceMap.get(socket.id);
      if (!voiceInfo) return;

      io.to(toSocketId).emit('voice:signal', {
        fromSocketId: socket.id,
        fromUserId: voiceInfo.userId,
        signalData,
      });
    });

    // Voice State Changes (Mute, Deafen, Speaking indicator)
    socket.on('voice:state-change', (updates: Partial<VoiceParticipant>) => {
      const voiceInfo = socketVoiceMap.get(socket.id);
      if (!voiceInfo) return;

      const room = voiceRooms.get(voiceInfo.channelId);
      if (!room) return;

      const participant = room.get(voiceInfo.userId);
      if (!participant) return;

      Object.assign(participant, updates);

      // Broadcast update to the voice room
      io.to(`voice:${voiceInfo.channelId}`).emit('voice:user-updated', {
        channelId: voiceInfo.channelId,
        participant,
      });

      // If mute/deafen changed, also update global sidebar
      if (updates.isMuted !== undefined || updates.isDeafened !== undefined) {
        io.emit('voice:global-state', getGlobalVoiceState());
      }
    });

    // 5. Disconnect
    socket.on('disconnect', () => {
      handleLeaveVoice(socket, io);
      const userId = socketUserMap.get(socket.id);
      if (userId) {
        socketUserMap.delete(socket.id);
      }
    });
  });
}

function handleLeaveVoice(socket: Socket, io: Server) {
  const voiceInfo = socketVoiceMap.get(socket.id);
  if (!voiceInfo) return;

  const { userId, channelId } = voiceInfo;
  socketVoiceMap.delete(socket.id);
  socket.leave(`voice:${channelId}`);

  const room = voiceRooms.get(channelId);
  if (room) {
    room.delete(userId);
    if (room.size === 0) {
      voiceRooms.delete(channelId);
    }
  }

  // Notify remaining peers in this room
  socket.to(`voice:${channelId}`).emit('voice:user-left', {
    channelId,
    socketId: socket.id,
    userId,
  });

  // Update global sidebar state
  io.emit('voice:global-state', getGlobalVoiceState());
}
