import React from 'react';
import {
  Hash,
  Volume2,
  Plus,
  Mic,
  MicOff,
  Headphones,
  Settings,
  PhoneOff,
  Radio,
  Sliders,
} from 'lucide-react';
import { Server, Channel, User } from '../../types';
import { useAuth } from '../../context/AuthContext';
import { useSocket } from '../../context/SocketContext';

interface ChannelListProps {
  server: Server | null;
  activeChannelId: string | null;
  onSelectChannel: (channel: Channel) => void;
  onCreateChannel: () => void;
  onOpenSettings: () => void;
}

export const ChannelList: React.FC<ChannelListProps> = ({
  server,
  activeChannelId,
  onSelectChannel,
  onCreateChannel,
  onOpenSettings,
}) => {
  const { user, demoUsers, switchUser } = useAuth();
  const {
    globalVoiceState,
    activeVoiceChannelId,
    speakingUsers,
    isMuted,
    isDeafened,
    joinVoiceChannel,
    leaveVoiceChannel,
    toggleMute,
    toggleDeafen,
  } = useSocket();

  if (!server) {
    return (
      <aside aria-label="Каналы сервера" className="w-60 bg-discord-sidebar flex flex-col h-full flex-shrink-0">
        <div className="h-12 border-b border-discord-dark flex items-center px-4 font-bold text-white shadow-sm">
          Загрузка...
        </div>
      </aside>
    );
  }

  const textChannels = server.channels.filter(c => c.type === 'text');
  const voiceChannels = server.channels.filter(c => c.type === 'voice');

  return (
    <aside aria-label="Каналы сервера" className="w-60 bg-discord-sidebar flex flex-col h-full flex-shrink-0 select-none">
      {/* Server Header */}
      <div className="h-12 border-b border-discord-dark flex items-center justify-between px-4 font-bold text-white shadow-sm hover:bg-discord-hover cursor-pointer transition">
        <span className="truncate">{server.name}</span>
        <button
          onClick={onCreateChannel}
          className="text-discord-text-muted hover:text-white transition p-1"
          title="Создать канал"
        >
          <Plus size={18} />
        </button>
      </div>

      {/* Channel Groups */}
      <div className="flex-1 overflow-y-auto px-2 py-3 space-y-4">
        {/* Text Channels */}
        <div>
          <div className="flex items-center justify-between text-[11px] font-bold text-discord-text-muted px-2 mb-1 tracking-wider uppercase">
            <span>Текстовые каналы</span>
          </div>
          <div className="space-y-[2px]">
            {textChannels.map(channel => {
              const isActive = activeChannelId === channel.id;
              return (
                <button
                  key={channel.id}
                  onClick={() => onSelectChannel(channel)}
                  className={`w-full flex items-center px-2 py-1.5 rounded text-sm font-medium transition group ${
                    isActive
                      ? 'bg-discord-active text-white'
                      : 'text-discord-text-muted hover:bg-discord-hover hover:text-discord-text-normal'
                  }`}
                >
                  <Hash size={18} className="mr-1.5 flex-shrink-0 text-discord-text-muted" />
                  <span className="truncate">{channel.name}</span>
                </button>
              );
            })}
          </div>
        </div>

        {/* Voice Channels */}
        <div>
          <div className="flex items-center justify-between text-[11px] font-bold text-discord-text-muted px-2 mb-1 tracking-wider uppercase">
            <span>Голосовые каналы</span>
          </div>
          <div className="space-y-[2px]">
            {voiceChannels.map(channel => {
              const isConnectedHere = activeVoiceChannelId === channel.id;
              const participants = globalVoiceState[channel.id] || [];

              return (
                <div key={channel.id} className="space-y-1">
                  <button
                    onClick={() => {
                      onSelectChannel(channel);
                      joinVoiceChannel(channel.id);
                    }}
                    className={`w-full flex items-center justify-between px-2 py-1.5 rounded text-sm font-medium transition group ${
                      isConnectedHere
                        ? 'bg-discord-active text-white'
                        : 'text-discord-text-muted hover:bg-discord-hover hover:text-discord-text-normal'
                    }`}
                  >
                    <div className="flex items-center truncate">
                      <Volume2
                        size={18}
                        className={`mr-1.5 flex-shrink-0 ${
                          isConnectedHere ? 'text-discord-green' : 'text-discord-text-muted'
                        }`}
                      />
                      <span className="truncate">{channel.name}</span>
                    </div>
                    {participants.length > 0 && (
                      <span className="text-xs bg-discord-dark px-1.5 py-0.5 rounded text-discord-text-muted">
                        {participants.length}
                      </span>
                    )}
                  </button>

                  {/* Connected users inside this voice channel */}
                  {participants.length > 0 && (
                    <div className="pl-6 pr-2 space-y-1 py-0.5">
                      {participants.map(p => {
                        const isSpeaking = speakingUsers[p.userId] || p.isSpeaking;
                        return (
                          <div
                            key={p.userId}
                            className="flex items-center justify-between py-1 px-2 rounded hover:bg-discord-hover text-xs text-discord-text-normal transition group"
                          >
                            <div className="flex items-center space-x-2 truncate">
                              <div className="relative">
                                <img
                                  src={p.avatar}
                                  alt={p.username}
                                  className={`w-6 h-6 rounded-full bg-discord-dark ${
                                    isSpeaking ? 'ring-2 ring-discord-green ring-offset-1 ring-offset-discord-sidebar' : ''
                                  }`}
                                />
                              </div>
                              <span className={`truncate ${isSpeaking ? 'text-discord-green font-semibold' : ''}`}>
                                {p.username}
                              </span>
                            </div>

                            <div className="flex items-center space-x-1 text-discord-text-muted">
                              {p.isMuted && <MicOff size={12} className="text-discord-red" />}
                              {p.isDeafened && <Headphones size={12} className="text-discord-red" />}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </div>

      {/* Connected Voice Channel Status Bar */}
      {activeVoiceChannelId && (
        <div className="bg-discord-dark/70 border-b border-discord-dark px-3 py-2 flex items-center justify-between">
          <div className="flex items-center space-x-2 truncate">
            <Radio size={16} className="text-discord-green animate-pulse flex-shrink-0" />
            <div className="truncate">
              <div className="text-xs font-bold text-discord-green truncate">Голос подключен</div>
              <div className="text-[11px] text-discord-text-muted truncate">WebRTC / 15мс</div>
            </div>
          </div>
          <button
            onClick={leaveVoiceChannel}
            className="p-1.5 rounded hover:bg-discord-red/20 text-discord-red transition"
            title="Отключиться от голосового канала"
          >
            <PhoneOff size={16} />
          </button>
        </div>
      )}

      {/* Quick Demo User Switcher (For local testing) */}
      <div className="bg-discord-dark/50 px-3 py-1.5 border-t border-discord-dark flex items-center justify-between text-xs">
        <span className="text-discord-text-muted flex items-center gap-1">
          <Sliders size={12} /> Тест юзер:
        </span>
        <select
          value={user?.id || ''}
          onChange={(e) => switchUser(e.target.value)}
          aria-label="Быстрое переключение тестового пользователя"
          className="bg-discord-sidebar text-discord-text-normal text-xs rounded px-1 py-0.5 border border-discord-hover outline-none cursor-pointer"
        >
          {demoUsers.map((u: User) => (
            <option key={u.id} value={u.id}>
              {u.username}
            </option>
          ))}
        </select>
      </div>

      {/* User Controls Footer */}
      <div className="h-[52px] bg-[#0c1711] border-t border-discord-border/50 px-2 flex items-center justify-between">
        {/* User Info */}
        <div className="flex items-center space-x-2 truncate p-1 rounded hover:bg-discord-hover cursor-pointer transition">
          <div className="relative">
            <img
              src={user?.avatar || 'https://api.dicebear.com/7.x/bottts/svg?seed=Me'}
              alt={user?.username}
              className="w-8 h-8 rounded-full bg-discord-dark"
            />
            <div className="absolute bottom-0 right-0 w-2.5 h-2.5 rounded-full bg-discord-green ring-2 ring-[#0c1711]" />
          </div>
          <div className="truncate leading-tight">
            <div className="text-sm font-semibold text-discord-text-heading truncate">{user?.username}</div>
            <div className="text-[11px] text-discord-text-muted truncate">#0001</div>
          </div>
        </div>

        {/* Audio Toggles */}
        <div className="flex items-center text-discord-text-muted">
          <button
            onClick={toggleMute}
            className={`p-1.5 rounded hover:bg-discord-hover transition ${
              isMuted ? 'text-discord-red' : 'hover:text-white'
            }`}
            title={isMuted ? 'Включить микрофон' : 'Отключить микрофон'}
          >
            {isMuted ? <MicOff size={18} /> : <Mic size={18} />}
          </button>

          <button
            onClick={toggleDeafen}
            className={`p-1.5 rounded hover:bg-discord-hover transition ${
              isDeafened ? 'text-discord-red' : 'hover:text-white'
            }`}
            title={isDeafened ? 'Включить звук' : 'Заглушить звук'}
          >
            <Headphones size={18} />
          </button>

          <button
            onClick={onOpenSettings}
            className="p-1.5 rounded hover:bg-discord-hover hover:text-white transition"
            title="Настройки звука и приложения"
          >
            <Settings size={18} />
          </button>
        </div>
      </div>
    </aside>
  );
};
