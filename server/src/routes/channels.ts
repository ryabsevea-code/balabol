import { Router, Request, Response } from 'express';
import { db, Channel } from '../db/store';

export const channelsRouter = Router();

// Get all servers with their channels
channelsRouter.get('/servers', (req: Request, res: Response) => {
  const servers = db.getServers().map(server => {
    const channels = db.getChannelsByServerId(server.id);
    return {
      ...server,
      channels,
    };
  });
  res.json({ servers });
});

// Create new channel
channelsRouter.post('/channels', (req: Request, res: Response) => {
  const { serverId, name, type, description } = req.body;

  if (!serverId || !name || !type) {
    return res.status(400).json({ error: 'serverId, name и type обязательны' });
  }

  const newChannel: Channel = {
    id: `c-${Date.now()}`,
    serverId,
    name: name.toLowerCase().replace(/\s+/g, '-'),
    type,
    description,
  };

  db.createChannel(newChannel);
  res.status(201).json({ channel: newChannel });
});

// Get messages for a specific channel
channelsRouter.get('/channels/:id/messages', (req: Request, res: Response) => {
  const channelId = String(req.params.id);
  const messages = db.getMessagesByChannelId(channelId);

  // Attach author information
  const enrichedMessages = messages.map(msg => {
    const author = db.getUserById(msg.authorId);
    return {
      ...msg,
      author: author ? {
        id: author.id,
        username: author.username,
        avatar: author.avatar,
        status: author.status,
      } : {
        id: msg.authorId,
        username: 'Неизвестный',
        avatar: '',
        status: 'offline',
      },
    };
  });

  res.json({ messages: enrichedMessages });
});

// Get DMs for a user
channelsRouter.get('/dms/:userId', (req: Request, res: Response) => {
  const userId = String(req.params.userId);
  const dms = db.getUserDMs(userId);

  const enrichedDMs = dms.map(dm => {
    const otherUserId = dm.participantIds.find(id => id !== userId) || dm.participantIds[0];
    const otherUser = db.getUserById(otherUserId);
    const lastMessages = db.getDMMessages(dm.id, 1);
    return {
      ...dm,
      otherUser: otherUser ? {
        id: otherUser.id,
        username: otherUser.username,
        avatar: otherUser.avatar,
        status: otherUser.status,
        customStatus: otherUser.customStatus,
      } : null,
      lastMessage: lastMessages[0] || null,
    };
  });

  res.json({ dms: enrichedDMs });
});

// Start or get DM with a user
channelsRouter.post('/dms', (req: Request, res: Response) => {
  const { userA, userB } = req.body;
  if (!userA || !userB) {
    return res.status(400).json({ error: 'userA и userB обязательны' });
  }

  const dm = db.getOrCreateDM(userA, userB);
  const otherUser = db.getUserById(userB);

  res.json({
    dm: {
      ...dm,
      otherUser: otherUser ? {
        id: otherUser.id,
        username: otherUser.username,
        avatar: otherUser.avatar,
        status: otherUser.status,
        customStatus: otherUser.customStatus,
      } : null,
    },
  });
});

// Get messages for a DM
channelsRouter.get('/dms/:id/messages', (req: Request, res: Response) => {
  const dmId = String(req.params.id);
  const messages = db.getDMMessages(dmId);

  const enrichedMessages = messages.map(msg => {
    const author = db.getUserById(msg.authorId);
    return {
      ...msg,
      author: author ? {
        id: author.id,
        username: author.username,
        avatar: author.avatar,
        status: author.status,
      } : {
        id: msg.authorId,
        username: 'Неизвестный',
        avatar: '',
        status: 'offline',
      },
    };
  });

  res.json({ messages: enrichedMessages });
});
