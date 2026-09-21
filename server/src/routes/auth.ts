import { Router, Request, Response } from 'express';
import bcrypt from 'bcryptjs';
import jwt from 'jsonwebtoken';
import { db, User } from '../db/store';

export const authRouter = Router();
const JWT_SECRET = process.env.JWT_SECRET || 'balabol-super-secret-key-2025';

function sanitizeUser(user: User) {
  const { passwordHash, ...safe } = user;
  return safe;
}

// Get all available demo users for quick switching
authRouter.get('/demo-users', (req: Request, res: Response) => {
  const users = db.getUsers().map(sanitizeUser);
  res.json({ users });
});

// Register
authRouter.post('/register', (req: Request, res: Response) => {
  const { username, email, password } = req.body;

  if (!username || !email || !password) {
    return res.status(400).json({ error: 'Все поля обязательны для заполнения' });
  }

  if (db.getUserByEmail(email)) {
    return res.status(400).json({ error: 'Пользователь с таким email уже существует' });
  }

  if (db.getUserByUsername(username)) {
    return res.status(400).json({ error: 'Имя пользователя уже занято' });
  }

  const passwordHash = bcrypt.hashSync(password, 8);
  const newUser: User = {
    id: `u-${Date.now()}`,
    username,
    email,
    passwordHash,
    avatar: `https://api.dicebear.com/7.x/bottts/svg?seed=${encodeURIComponent(username)}`,
    status: 'online',
    createdAt: new Date().toISOString(),
  };

  db.createUser(newUser);

  const token = jwt.sign({ id: newUser.id }, JWT_SECRET, { expiresIn: '30d' });
  res.status(201).json({
    token,
    user: sanitizeUser(newUser),
  });
});

// Login
authRouter.post('/login', (req: Request, res: Response) => {
  const { emailOrUsername, password } = req.body;

  if (!emailOrUsername || !password) {
    return res.status(400).json({ error: 'Введите логин/email и пароль' });
  }

  const user = db.getUserByEmail(emailOrUsername) || db.getUserByUsername(emailOrUsername);

  if (!user || !bcrypt.compareSync(password, user.passwordHash)) {
    return res.status(401).json({ error: 'Неверный логин или пароль' });
  }

  db.updateUserStatus(user.id, 'online');

  const token = jwt.sign({ id: user.id }, JWT_SECRET, { expiresIn: '30d' });
  res.json({
    token,
    user: sanitizeUser(user),
  });
});

// Quick switch to a demo user
authRouter.post('/switch-user', (req: Request, res: Response) => {
  const { userId } = req.body;
  const user = db.getUserById(userId);

  if (!user) {
    return res.status(404).json({ error: 'Пользователь не найден' });
  }

  db.updateUserStatus(user.id, 'online');

  const token = jwt.sign({ id: user.id }, JWT_SECRET, { expiresIn: '30d' });
  res.json({
    token,
    user: sanitizeUser(user),
  });
});

// Current User
authRouter.get('/me', (req: Request, res: Response) => {
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    return res.status(401).json({ error: 'Не авторизован' });
  }

  const token = authHeader.split(' ')[1];
  try {
    const payload = jwt.verify(token, JWT_SECRET) as { id: string };
    const user = db.getUserById(payload.id);
    if (!user) {
      return res.status(404).json({ error: 'Пользователь не найден' });
    }
    res.json({ user: sanitizeUser(user) });
  } catch (err) {
    res.status(401).json({ error: 'Недействительный токен' });
  }
});
