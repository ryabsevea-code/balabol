import React, { createContext, useContext, useState, useEffect } from 'react';
import { User } from '../types';
import { API_BASE } from '../utils/config';

interface AuthContextType {
  user: User | null;
  token: string | null;
  isLoading: boolean;
  demoUsers: User[];
  login: (emailOrUsername: string, password: string) => Promise<void>;
  register: (username: string, email: string, password: string) => Promise<void>;
  switchUser: (userId: string) => Promise<void>;
  logout: () => void;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

const AUTH_API = `${API_BASE}/auth`;

export const AuthProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [user, setUser] = useState<User | null>(null);
  const [token, setToken] = useState<string | null>(localStorage.getItem('balabol_token'));
  const [isLoading, setIsLoading] = useState(true);
  const [demoUsers, setDemoUsers] = useState<User[]>([]);

  // Fetch demo users for easy test switching
  const fetchDemoUsers = async () => {
    try {
      const res = await fetch(`${AUTH_API}/demo-users`);
      if (res.ok) {
        const data = await res.json();
        setDemoUsers(data.users);
      }
    } catch (e) {
      console.error('Failed to load demo users', e);
    }
  };

  // Verify current token
  useEffect(() => {
    fetchDemoUsers();
    if (!token) {
      setIsLoading(false);
      return;
    }

    fetch(`${AUTH_API}/me`, {
      headers: { Authorization: `Bearer ${token}` },
    })
      .then(res => res.ok ? res.json() : Promise.reject())
      .then(data => {
        setUser(data.user);
      })
      .catch(() => {
        localStorage.removeItem('balabol_token');
        setToken(null);
        setUser(null);
      })
      .finally(() => {
        setIsLoading(false);
      });
  }, [token]);

  const login = async (emailOrUsername: string, password: string) => {
    const res = await fetch(`${AUTH_API}/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ emailOrUsername, password }),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || 'Ошибка входа');

    localStorage.setItem('balabol_token', data.token);
    setToken(data.token);
    setUser(data.user);
  };

  const register = async (username: string, email: string, password: string) => {
    const res = await fetch(`${AUTH_API}/register`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, email, password }),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || 'Ошибка регистрации');

    localStorage.setItem('balabol_token', data.token);
    setToken(data.token);
    setUser(data.user);
  };

  const switchUser = async (userId: string) => {
    const res = await fetch(`${AUTH_API}/switch-user`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ userId }),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || 'Ошибка переключения');

    localStorage.setItem('balabol_token', data.token);
    setToken(data.token);
    setUser(data.user);
  };

  const logout = () => {
    localStorage.removeItem('balabol_token');
    setToken(null);
    setUser(null);
  };

  return (
    <AuthContext.Provider value={{ user, token, isLoading, demoUsers, login, register, switchUser, logout }}>
      {children}
    </AuthContext.Provider>
  );
};

export const useAuth = () => {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within an AuthProvider');
  return ctx;
};
