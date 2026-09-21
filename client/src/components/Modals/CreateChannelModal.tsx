import React, { useState } from 'react';
import { X, Hash, Volume2 } from 'lucide-react';
import { API_BASE } from '../../utils/config';

interface CreateChannelModalProps {
  isOpen: boolean;
  serverId: string;
  onClose: () => void;
  onChannelCreated: () => void;
}

export const CreateChannelModal: React.FC<CreateChannelModalProps> = ({
  isOpen,
  serverId,
  onClose,
  onChannelCreated,
}) => {
  const [name, setName] = useState('');
  const [type, setType] = useState<'text' | 'voice'>('text');
  const [description, setDescription] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!isOpen) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;

    setIsSubmitting(true);
    setError(null);

    try {
      const res = await fetch(`${API_BASE}/channels`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          serverId,
          name: name.trim(),
          type,
          description: description.trim() || undefined,
        }),
      });

      if (!res.ok) {
        const data = await res.json();
        throw new Error(data.error || 'Не удалось создать канал');
      }

      setName('');
      setDescription('');
      onChannelCreated();
      onClose();
    } catch (err: any) {
      setError(err.message);
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/75 flex items-center justify-center p-4 backdrop-blur-sm">
      <div className="bg-[#14251c] w-full max-w-md rounded-xl shadow-2xl overflow-hidden border border-discord-border">
        <div className="p-6">
          <div className="flex items-center justify-between mb-4">
            <h2 className="text-xl font-bold text-white">Создать канал</h2>
            <button
              onClick={onClose}
              className="text-discord-text-muted hover:text-white transition"
            >
              <X size={20} />
            </button>
          </div>

          {error && (
            <div className="mb-4 p-3 rounded bg-discord-red/10 border border-discord-red/30 text-discord-red text-xs">
              {error}
            </div>
          )}

          <form onSubmit={handleSubmit} className="space-y-4">
            {/* Channel Type */}
            <div className="space-y-2">
              <label className="text-xs font-bold text-discord-text-muted uppercase tracking-wider">
                Тип канала
              </label>

              {/* Text Option */}
              <div
                onClick={() => setType('text')}
                className={`p-3 rounded-lg flex items-center justify-between cursor-pointer transition border ${
                  type === 'text'
                    ? 'bg-[#1b3124] border-discord-blurple text-white shadow-emerald-glow'
                    : 'bg-[#101e16] border-discord-border/50 text-discord-text-muted hover:bg-[#162a1f]'
                }`}
              >
                <div className="flex items-center space-x-3">
                  <Hash size={24} className={type === 'text' ? 'text-discord-blurple' : 'text-discord-text-muted'} />
                  <div>
                    <div className="text-sm font-semibold">Текстовый канал</div>
                    <div className="text-xs text-discord-text-muted">Отправляйте сообщения, картинки и файлы</div>
                  </div>
                </div>
                <input
                  type="radio"
                  name="channelType"
                  checked={type === 'text'}
                  onChange={() => setType('text')}
                  className="accent-discord-blurple"
                />
              </div>

              {/* Voice Option */}
              <div
                onClick={() => setType('voice')}
                className={`p-3 rounded-lg flex items-center justify-between cursor-pointer transition border ${
                  type === 'voice'
                    ? 'bg-[#1b3124] border-discord-blurple text-white shadow-emerald-glow'
                    : 'bg-[#101e16] border-discord-border/50 text-discord-text-muted hover:bg-[#162a1f]'
                }`}
              >
                <div className="flex items-center space-x-3">
                  <Volume2 size={24} className={type === 'voice' ? 'text-discord-blurple' : 'text-discord-text-muted'} />
                  <div>
                    <div className="text-sm font-semibold">Голосовой канал</div>
                    <div className="text-xs text-discord-text-muted">Общайтесь голосом с шумодавом и WebRTC</div>
                  </div>
                </div>
                <input
                  type="radio"
                  name="channelType"
                  checked={type === 'voice'}
                  onChange={() => setType('voice')}
                  className="accent-discord-blurple"
                />
              </div>
            </div>

            {/* Channel Name */}
            <div className="space-y-1.5">
              <label className="text-xs font-bold text-discord-text-muted uppercase tracking-wider">
                Название канала
              </label>
              <div className="relative flex items-center">
                {type === 'text' ? (
                  <Hash size={18} className="absolute left-3 text-discord-text-muted" />
                ) : (
                  <Volume2 size={18} className="absolute left-3 text-discord-text-muted" />
                )}
                <input
                  type="text"
                  value={name}
                  onChange={e => setName(e.target.value)}
                  placeholder="новый-канал"
                  className="w-full bg-[#0e1b13] text-sm text-white pl-9 pr-3 py-2 rounded-lg outline-none focus:ring-1 focus:ring-discord-blurple border border-discord-border/50 placeholder-discord-text-muted"
                  required
                />
              </div>
            </div>

            {/* Description */}
            <div className="space-y-1.5">
              <label className="text-xs font-bold text-discord-text-muted uppercase tracking-wider">
                Тема канала (необязательно)
              </label>
              <input
                type="text"
                value={description}
                onChange={e => setDescription(e.target.value)}
                placeholder="Для чего этот канал..."
                className="w-full bg-[#0e1b13] text-sm text-white px-3 py-2 rounded-lg outline-none focus:ring-1 focus:ring-discord-blurple border border-discord-border/50 placeholder-discord-text-muted"
              />
            </div>

            {/* Footer Buttons */}
            <div className="pt-4 flex items-center justify-end space-x-3">
              <button
                type="button"
                onClick={onClose}
                className="px-4 py-2 text-sm font-medium text-white hover:underline transition"
              >
                Отмена
              </button>
              <button
                type="submit"
                disabled={!name.trim() || isSubmitting}
                className="bg-discord-blurple hover:bg-discord-blurple-hover disabled:opacity-50 text-white px-5 py-2 rounded-lg text-sm font-semibold transition shadow"
              >
                {isSubmitting ? 'Создание...' : 'Создать канал'}
              </button>
            </div>
          </form>
        </div>
      </div>
    </div>
  );
};
