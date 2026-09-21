import React, { useState } from 'react';
import { MessageSquare, User as UserIcon, Lock, Mail, ArrowRight } from 'lucide-react';
import { useAuth } from '../../context/AuthContext';
import { User } from '../../types';

export const AuthModal: React.FC = () => {
  const { user, login, register, switchUser, demoUsers } = useAuth();
  const [isRegister, setIsRegister] = useState(false);
  const [username, setUsername] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  if (user) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    setLoading(true);

    try {
      if (isRegister) {
        await register(username, email, password);
      } else {
        await login(username || email, password);
      }
    } catch (err: any) {
      setError(err.message || 'Ошибка аутентификации');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-[#09130e] flex items-center justify-center p-4">
      <div className="bg-[#132219] w-full max-w-md rounded-2xl shadow-2xl p-8 border border-discord-border space-y-6">
        {/* Header */}
        <div className="text-center space-y-1">
          <div className="w-14 h-14 bg-discord-blurple text-white rounded-2xl flex items-center justify-center mx-auto mb-3 shadow-lg shadow-discord-blurple/25">
            <MessageSquare size={32} />
          </div>
          <h1 className="text-2xl font-extrabold text-white">Balabol</h1>
          <p className="text-sm text-discord-text-muted">
            {isRegister ? 'Создать новый аккаунт' : 'С возвращением! Войдите в аккаунт'}
          </p>
        </div>

        {/* 1-Click Demo Login (Super convenient for testing!) */}
        <div className="bg-[#0c1811] p-4 rounded-xl border border-discord-border/60 space-y-2.5">
          <div className="text-xs font-bold text-discord-text-muted uppercase tracking-wider flex items-center justify-between">
            <span>Быстрый вход для тестов</span>
            <span className="text-[10px] bg-discord-blurple/20 text-discord-blurple px-1.5 py-0.5 rounded font-semibold">
              1 клик
            </span>
          </div>
          <div className="grid grid-cols-1 gap-1.5">
            {demoUsers.map((u: User) => (
              <button
                key={u.id}
                onClick={() => switchUser(u.id)}
                className="flex items-center justify-between px-3 py-2 rounded-lg bg-[#182c21] hover:bg-[#1f3729] text-white text-xs font-semibold transition group border border-discord-border/40 hover:border-discord-blurple/50"
              >
                <div className="flex items-center space-x-2.5">
                  <img src={u.avatar} alt={u.username} className="w-6 h-6 rounded-full bg-discord-dark" />
                  <span>{u.username}</span>
                </div>
                <ArrowRight size={14} className="text-discord-text-muted group-hover:text-discord-blurple transition" />
              </button>
            ))}
          </div>
        </div>

        {error && (
          <div className="p-3 rounded-lg bg-discord-red/10 border border-discord-red/30 text-discord-red text-xs">
            {error}
          </div>
        )}

        {/* Regular Login/Register Form */}
        <form onSubmit={handleSubmit} className="space-y-4">
          {isRegister && (
            <div className="space-y-1">
              <label className="text-xs font-bold text-discord-text-muted uppercase">Имя пользователя</label>
              <div className="relative flex items-center">
                <UserIcon size={16} className="absolute left-3 text-discord-text-muted" />
                <input
                  type="text"
                  value={username}
                  onChange={e => setUsername(e.target.value)}
                  placeholder="CoolGamer"
                  className="w-full bg-[#0c1811] text-sm text-white pl-9 pr-3 py-2 rounded-lg outline-none focus:ring-1 focus:ring-discord-blurple border border-discord-border/50"
                  required
                />
              </div>
            </div>
          )}

          <div className="space-y-1">
            <label className="text-xs font-bold text-discord-text-muted uppercase">
              {isRegister ? 'Email' : 'Имя пользователя или Email'}
            </label>
            <div className="relative flex items-center">
              <Mail size={16} className="absolute left-3 text-discord-text-muted" />
              <input
                type={isRegister ? 'email' : 'text'}
                value={isRegister ? email : username}
                onChange={e => isRegister ? setEmail(e.target.value) : setUsername(e.target.value)}
                placeholder={isRegister ? 'user@example.com' : 'alex@balabol.local или Alex_Gamer'}
                className="w-full bg-[#0c1811] text-sm text-white pl-9 pr-3 py-2 rounded-lg outline-none focus:ring-1 focus:ring-discord-blurple border border-discord-border/50"
                required
              />
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-xs font-bold text-discord-text-muted uppercase">Пароль</label>
            <div className="relative flex items-center">
              <Lock size={16} className="absolute left-3 text-discord-text-muted" />
              <input
                type="password"
                value={password}
                onChange={e => setPassword(e.target.value)}
                placeholder="•••••• (по умолч. 123456)"
                className="w-full bg-[#0c1811] text-sm text-white pl-9 pr-3 py-2 rounded-lg outline-none focus:ring-1 focus:ring-discord-blurple border border-discord-border/50"
                required
              />
            </div>
          </div>

          <button
            type="submit"
            disabled={loading}
            className="w-full bg-discord-blurple hover:bg-discord-blurple-hover text-white py-2.5 rounded-lg font-bold text-sm transition shadow-lg shadow-discord-blurple/30 disabled:opacity-50"
          >
            {loading ? 'Загрузка...' : isRegister ? 'Зарегистрироваться' : 'Войти'}
          </button>
        </form>

        <div className="text-center text-xs text-discord-text-muted">
          {isRegister ? (
            <span>
              Уже есть аккаунт?{' '}
              <button
                onClick={() => setIsRegister(false)}
                className="text-discord-blurple font-semibold hover:underline"
              >
                Войти
              </button>
            </span>
          ) : (
            <span>
              Нужен аккаунт?{' '}
              <button
                onClick={() => setIsRegister(true)}
                className="text-discord-blurple font-semibold hover:underline"
              >
                Зарегистрироваться
              </button>
            </span>
          )}
        </div>
      </div>
    </div>
  );
};
