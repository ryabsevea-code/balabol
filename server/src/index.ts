import express from 'express';
import http from 'http';
import { Server } from 'socket.io';
import cors from 'cors';
import dotenv from 'dotenv';
import path from 'path';
import { authRouter } from './routes/auth';
import { channelsRouter } from './routes/channels';
import { registerSocketHandlers } from './socket';

dotenv.config();

const app = express();
const server = http.createServer(app);

const PORT = process.env.PORT || 3001;

// CORS setup
app.use(cors({
  origin: '*',
  methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
  allowedHeaders: ['Content-Type', 'Authorization'],
}));

app.use(express.json());

// Public data folder for uploads / avatars
const uploadsDir = path.join(__dirname, '../data/uploads');
app.use('/uploads', express.static(uploadsDir));

// API Routes
app.use('/api/auth', authRouter);
app.use('/api', channelsRouter);

// Health check
app.get('/api/health', (req, res) => {
  res.json({ status: 'ok', app: 'Balabol Server', timestamp: new Date().toISOString() });
});

// Serve frontend build if exists (for single-port home server deployment)
const clientDistPath = path.resolve(__dirname, '../../client/dist');
if (require('fs').existsSync(clientDistPath)) {
  console.log(`📦 Обнаружен собранный клиент в ${clientDistPath}, включаем статическую раздачу`);
  app.use(express.static(clientDistPath));
  app.get('*', (req, res, next) => {
    if (req.path.startsWith('/api') || req.path.startsWith('/uploads') || req.path.startsWith('/socket.io')) {
      return next();
    }
    res.sendFile(path.join(clientDistPath, 'index.html'));
  });
}

// Socket.io initialization with CORS
const io = new Server(server, {
  cors: {
    origin: '*',
    methods: ['GET', 'POST'],
  },
  pingTimeout: 60000,
});

registerSocketHandlers(io);

server.listen(PORT, () => {
  console.log(`===========================================`);
  console.log(`🚀 Balabol Server запущен на http://localhost:${PORT}`);
  console.log(`📡 WebSocket шлюз и WebRTC сигналинг готовы`);
  console.log(`===========================================`);
});
