import React from 'react';
import {
  Mic,
  MicOff,
  Headphones,
  Volume2,
  VolumeX,
  ShieldCheck,
  PhoneOff,
  Users,
} from 'lucide-react';
import { Channel } from '../../types';
import { useAuth } from '../../context/AuthContext';
import { useSocket } from '../../context/SocketContext';

interface VoiceGridProps {
  channel: Channel;
}

export const VoiceGrid: React.FC<VoiceGridProps> = ({ channel }) => {
  const { user } = useAuth();
  const {
    voiceParticipants,
    speakingUsers,
    activeVoiceChannelId,
    joinVoiceChannel,
    leaveVoiceChannel,
    isMuted,
    isDeafened,
    setUserVolume,
    getUserVolume,
    voiceConfig,
    updateVoiceConfig,
  } = useSocket();

  const isConnected = activeVoiceChannelId === channel.id;

  // Add local user to participants list if in this channel
  const allParticipants = React.useMemo(() => {
    if (!isConnected || !user) return voiceParticipants;

    const hasSelf = voiceParticipants.some(p => p.userId === user.id);
    if (!hasSelf) {
      return [
        {
          userId: user.id,
          socketId: 'local',
          username: user.username,
          avatar: user.avatar,
          isMuted,
          isDeafened,
          isSpeaking: speakingUsers[user.id] || false,
        },
        ...voiceParticipants,
      ];
    }
    return voiceParticipants;
  }, [isConnected, user, voiceParticipants, isMuted, isDeafened, speakingUsers]);

  return (
    <div className="flex-1 bg-discord-chat flex flex-col h-full overflow-hidden">
      {/* Voice Header Bar */}
      <div className="h-12 border-b border-discord-dark px-4 flex items-center justify-between shadow-sm bg-discord-chat/90 backdrop-blur">
        <div className="flex items-center space-x-2">
          <Volume2 size={24} className="text-discord-green" />
          <h1 className="font-bold text-white text-base">{channel.name}</h1>
          <span className="text-xs text-discord-text-muted bg-discord-dark px-2 py-0.5 rounded">
            Голосовой канал
          </span>
        </div>

        {/* Quick controls in header */}
        <div className="flex items-center space-x-3">
          {/* Noise suppression toggle */}
          <button
            onClick={() => updateVoiceConfig({ noiseSuppression: !voiceConfig.noiseSuppression })}
            className={`flex items-center space-x-1.5 px-2.5 py-1 rounded text-xs font-semibold transition ${
              voiceConfig.noiseSuppression
                ? 'bg-discord-green/20 text-discord-green border border-discord-green/30'
                : 'bg-discord-dark text-discord-text-muted hover:text-white'
            }`}
            title="Шумоподавление (Шумодав)"
          >
            <ShieldCheck size={14} />
            <span>Шумодав: {voiceConfig.noiseSuppression ? 'ВКЛ' : 'ВЫКЛ'}</span>
          </button>

          {/* Connect / Disconnect button */}
          {isConnected ? (
            <button
              onClick={leaveVoiceChannel}
              className="flex items-center space-x-1.5 bg-discord-red hover:bg-red-600 text-white px-3 py-1 rounded text-xs font-bold transition shadow"
            >
              <PhoneOff size={14} />
              <span>Отключиться</span>
            </button>
          ) : (
            <button
              onClick={() => joinVoiceChannel(channel.id)}
              className="flex items-center space-x-1.5 bg-discord-green hover:bg-green-600 text-white px-3 py-1 rounded text-xs font-bold transition shadow"
            >
              <Volume2 size={14} />
              <span>Подключиться к голосу</span>
            </button>
          )}
        </div>
      </div>

      {/* Voice Grid Content */}
      <div className="flex-1 overflow-y-auto p-6 flex flex-col justify-center">
        {allParticipants.length === 0 ? (
          <div className="text-center py-12 space-y-4 max-w-sm mx-auto">
            <div className="w-16 h-16 bg-discord-dark rounded-full flex items-center justify-center mx-auto text-discord-text-muted">
              <Users size={32} />
            </div>
            <h3 className="text-lg font-bold text-white">В канале пока никого нет</h3>
            <p className="text-sm text-discord-text-muted">
              Нажмите кнопку «Подключиться к голосу» выше, чтобы начать общение с друзьями!
            </p>
            <button
              onClick={() => joinVoiceChannel(channel.id)}
              className="bg-discord-blurple hover:bg-discord-blurple-hover text-white px-5 py-2 rounded font-semibold text-sm transition"
            >
              Войти в канал
            </button>
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4 max-w-6xl mx-auto w-full">
            {allParticipants.map(participant => {
              const isSelf = participant.userId === user?.id;
              const isSpeaking = speakingUsers[participant.userId] || participant.isSpeaking;
              const volume = isSelf ? 1.0 : getUserVolume(participant.userId);
              const volumePercent = Math.round(volume * 100);

              return (
                <div
                  key={participant.userId}
                  className={`bg-[#16291e] rounded-xl p-4 flex flex-col items-center justify-between relative shadow-xl border transition-all duration-200 ${
                    isSpeaking
                      ? 'border-discord-green ring-2 ring-discord-green/50 shadow-emerald-glow'
                      : 'border-discord-border/60 hover:border-discord-blurple/40'
                  }`}
                >
                  {/* Status Badges top right */}
                  <div className="absolute top-3 right-3 flex items-center space-x-1.5">
                    {participant.isMuted && (
                      <div className="bg-discord-red/20 text-discord-red p-1 rounded-full" title="Микрофон выключен">
                        <MicOff size={14} />
                      </div>
                    )}
                    {participant.isDeafened && (
                      <div className="bg-discord-red/20 text-discord-red p-1 rounded-full" title="Звук заглушен">
                        <Headphones size={14} />
                      </div>
                    )}
                  </div>

                  {/* Avatar with speaking glow */}
                  <div className="relative my-3">
                    <img
                      src={participant.avatar}
                      alt={participant.username}
                      className={`w-20 h-20 rounded-full bg-discord-dark object-cover transition-transform duration-200 ${
                        isSpeaking ? 'speaking-pulse scale-105' : ''
                      }`}
                    />
                    {isSpeaking && (
                      <span className="absolute -bottom-1 left-1/2 transform -translate-x-1/2 bg-discord-green text-black font-extrabold text-[10px] px-2 py-0.2 rounded-full uppercase tracking-wider">
                        Говорит
                      </span>
                    )}
                  </div>

                  {/* Username */}
                  <div className="text-center w-full mb-3">
                    <div className="font-bold text-white text-base truncate flex items-center justify-center space-x-1">
                      <span>{participant.username}</span>
                      {isSelf && <span className="text-xs text-discord-text-muted font-normal">(Вы)</span>}
                    </div>
                  </div>

                  {/* Individual Volume Slider (Per-User Volume Control) */}
                  {!isSelf ? (
                    <div className="w-full bg-[#0e1b13] rounded-lg p-2.5 space-y-1.5 border border-discord-border/50">
                      <div className="flex items-center justify-between text-xs text-discord-text-muted">
                        <span className="flex items-center gap-1 font-medium">
                          {volume === 0 ? (
                            <VolumeX size={14} className="text-discord-red" />
                          ) : (
                            <Volume2 size={14} className="text-discord-green" />
                          )}
                          Громкость
                        </span>
                        <span className="font-bold text-discord-text-heading">{volumePercent}%</span>
                      </div>

                      <input
                        type="range"
                        min="0"
                        max="2"
                        step="0.05"
                        value={volume}
                        onChange={e => setUserVolume(participant.userId, parseFloat(e.target.value))}
                        className="w-full h-1.5 bg-discord-hover rounded-lg appearance-none cursor-pointer accent-discord-blurple"
                        title={`Регулировка громкости пользователя ${participant.username}`}
                      />

                      <div className="flex justify-between text-[10px] text-discord-text-muted pt-0.5">
                        <button
                          onClick={() => setUserVolume(participant.userId, 0)}
                          className="hover:text-discord-red transition"
                        >
                          Заглушить
                        </button>
                        <button
                          onClick={() => setUserVolume(participant.userId, 1.0)}
                          className="hover:text-white transition"
                        >
                          100%
                        </button>
                        <button
                          onClick={() => setUserVolume(participant.userId, 2.0)}
                          className="hover:text-discord-green transition"
                        >
                          200%
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="w-full bg-[#0e1b13] rounded-lg p-2 text-center text-xs text-discord-text-muted border border-discord-border/50">
                      <span>Ваш микрофон {isMuted ? 'выключен' : 'активен'}</span>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
};
