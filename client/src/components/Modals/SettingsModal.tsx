import React, { useState, useEffect } from 'react';
import {
  X,
  Mic,
  ShieldCheck,
  Volume2,
  Sliders,
  LogOut,
  UserCheck,
} from 'lucide-react';
import { useAuth } from '../../context/AuthContext';
import { useSocket } from '../../context/SocketContext';
import { User } from '../../types';

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ isOpen, onClose }) => {
  const { user, demoUsers, switchUser, logout } = useAuth();
  const { voiceConfig, updateVoiceConfig } = useSocket();
  const [activeTab, setActiveTab] = useState<'voice' | 'account'>('voice');
  const [testMicLevel, setTestMicLevel] = useState(0);

  // Microphone level test meter
  useEffect(() => {
    if (!isOpen || activeTab !== 'voice') return;

    let audioCtx: AudioContext | null = null;
    let stream: MediaStream | null = null;
    let animId: number;

    navigator.mediaDevices?.getUserMedia({ audio: true })
      .then(s => {
        stream = s;
        const AudioCtx = window.AudioContext || (window as any).webkitAudioContext;
        audioCtx = new AudioCtx();
        const source = audioCtx.createMediaStreamSource(s);
        const analyser = audioCtx.createAnalyser();
        analyser.fftSize = 256;
        source.connect(analyser);

        const dataArray = new Uint8Array(analyser.frequencyBinCount);
        const updateMeter = () => {
          analyser.getByteFrequencyData(dataArray);
          let sum = 0;
          for (let i = 0; i < dataArray.length; i++) {
            sum += dataArray[i];
          }
          const avg = sum / dataArray.length;
          setTestMicLevel(Math.min(100, Math.round((avg / 128) * 100)));
          animId = requestAnimationFrame(updateMeter);
        };
        updateMeter();
      })
      .catch(() => {
        // Mic unavailable for test
      });

    return () => {
      cancelAnimationFrame(animId);
      stream?.getTracks().forEach(t => t.stop());
      audioCtx?.close();
    };
  }, [isOpen, activeTab]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 bg-black/75 flex items-center justify-center p-4 backdrop-blur-sm animate-fadeIn">
      <div className="bg-[#14251c] w-full max-w-2xl rounded-2xl shadow-2xl overflow-hidden flex flex-col md:flex-row max-h-[85vh] border border-discord-border">
        {/* Left Tabs Sidebar */}
        <div className="w-full md:w-52 bg-[#0e1b13] p-4 flex flex-col justify-between border-r border-discord-border/60">
          <div className="space-y-1">
            <div className="text-[11px] font-bold text-discord-text-muted px-2 mb-2 uppercase tracking-wider">
              Настройки
            </div>
            <button
              onClick={() => setActiveTab('voice')}
              className={`w-full flex items-center space-x-2.5 px-3 py-2 rounded text-sm font-semibold transition ${
                activeTab === 'voice'
                  ? 'bg-discord-active text-white'
                  : 'text-discord-text-muted hover:bg-discord-hover hover:text-white'
              }`}
            >
              <Mic size={18} />
              <span>Голос и звук</span>
            </button>
            <button
              onClick={() => setActiveTab('account')}
              className={`w-full flex items-center space-x-2.5 px-3 py-2 rounded text-sm font-semibold transition ${
                activeTab === 'account'
                  ? 'bg-discord-active text-white'
                  : 'text-discord-text-muted hover:bg-discord-hover hover:text-white'
              }`}
            >
              <UserCheck size={18} />
              <span>Мой аккаунт</span>
            </button>
          </div>

          <div className="pt-4 border-t border-discord-dark">
            <button
              onClick={() => {
                logout();
                onClose();
              }}
              className="w-full flex items-center space-x-2 px-3 py-2 rounded text-sm font-semibold text-discord-red hover:bg-discord-red/10 transition"
            >
              <LogOut size={16} />
              <span>Выйти из аккаунта</span>
            </button>
          </div>
        </div>

        {/* Right Content */}
        <div className="flex-1 p-6 overflow-y-auto relative flex flex-col justify-between">
          <button
            onClick={onClose}
            className="absolute top-4 right-4 p-1.5 rounded-full hover:bg-discord-hover text-discord-text-muted hover:text-white transition"
          >
            <X size={20} />
          </button>

          {activeTab === 'voice' && (
            <div className="space-y-6">
              <div>
                <h2 className="text-xl font-extrabold text-white flex items-center gap-2">
                  <Mic className="text-discord-blurple" /> Настройки звука и шумодава
                </h2>
                <p className="text-xs text-discord-text-muted mt-1">
                  Управление параметрами микрофона, фильтрации шумов и чувствительности.
                </p>
              </div>

              {/* Noise Suppression Switch */}
              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 flex items-center justify-between">
                <div className="space-y-0.5 pr-4">
                  <div className="font-bold text-white text-sm flex items-center gap-2">
                    <ShieldCheck size={16} className="text-discord-green" />
                    Шумоподавление (Шумодав)
                  </div>
                  <div className="text-xs text-discord-text-muted">
                    Отсекает шум клавиатуры, дыхания и вентиляторов ПК.
                  </div>
                </div>
                <label className="relative inline-flex items-center cursor-pointer">
                  <input
                    type="checkbox"
                    checked={voiceConfig.noiseSuppression}
                    onChange={e => updateVoiceConfig({ noiseSuppression: e.target.checked })}
                    className="sr-only peer"
                  />
                  <div className="w-11 h-6 bg-discord-hover peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-discord-green"></div>
                </label>
              </div>

              {/* Echo Cancellation Switch */}
              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 flex items-center justify-between">
                <div className="space-y-0.5 pr-4">
                  <div className="font-bold text-white text-sm flex items-center gap-2">
                    <Volume2 size={16} className="text-discord-blurple" />
                    Эхоподавление (Acoustic Echo Cancellation)
                  </div>
                  <div className="text-xs text-discord-text-muted">
                    Предотвращает попадание звука из ваших динамиков обратно в микрофон.
                  </div>
                </div>
                <label className="relative inline-flex items-center cursor-pointer">
                  <input
                    type="checkbox"
                    checked={voiceConfig.echoCancellation}
                    onChange={e => updateVoiceConfig({ echoCancellation: e.target.checked })}
                    className="sr-only peer"
                  />
                  <div className="w-11 h-6 bg-discord-hover peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-discord-green"></div>
                </label>
              </div>

              {/* Microphone Test */}
              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 space-y-2">
                <div className="flex items-center justify-between text-sm">
                  <span className="font-bold text-white">Проверка микрофона</span>
                  <span className="text-xs text-discord-text-muted">{testMicLevel}%</span>
                </div>
                <div className="w-full h-3 bg-discord-dark rounded-full overflow-hidden p-0.5">
                  <div
                    className="h-full bg-gradient-to-r from-discord-green via-discord-yellow to-discord-red rounded-full transition-all duration-75"
                    style={{ width: `${testMicLevel}%` }}
                  />
                </div>
                <div className="text-[11px] text-discord-text-muted">
                  Произнесите что-нибудь — полоска должна двигаться.
                </div>
              </div>

              {/* Sensitivity Slider */}
              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 space-y-2">
                <div className="flex items-center justify-between text-sm">
                  <span className="font-bold text-white">Порог активации по голосу</span>
                  <span className="text-xs text-discord-text-heading font-semibold">{voiceConfig.sensitivityThreshold}</span>
                </div>
                <input
                  type="range"
                  min="5"
                  max="80"
                  value={voiceConfig.sensitivityThreshold}
                  onChange={e => updateVoiceConfig({ sensitivityThreshold: parseInt(e.target.value) })}
                  className="w-full h-1.5 bg-discord-dark rounded-lg appearance-none cursor-pointer accent-discord-blurple"
                />
                <div className="text-[11px] text-discord-text-muted">
                  Чем ниже значение, тем чувствительнее микрофон реагирует на тихий шепот.
                </div>
              </div>
            </div>
          )}

          {activeTab === 'account' && (
            <div className="space-y-6">
              <div>
                <h2 className="text-xl font-extrabold text-white">Мой аккаунт</h2>
                <p className="text-xs text-discord-text-muted mt-1">Информация профиля и быстрое переключение.</p>
              </div>

              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 flex items-center space-x-4">
                <img
                  src={user?.avatar}
                  alt={user?.username}
                  className="w-16 h-16 rounded-full bg-discord-dark border-2 border-discord-blurple"
                />
                <div>
                  <div className="text-lg font-bold text-white">{user?.username}</div>
                  <div className="text-xs text-discord-text-muted">{user?.email}</div>
                  <div className="text-xs text-discord-green mt-1 font-semibold">● В сети</div>
                </div>
              </div>

              <div className="bg-[#182c21] p-4 rounded-xl border border-discord-border/60 space-y-3">
                <div className="text-sm font-bold text-white flex items-center gap-1.5">
                  <Sliders size={16} /> Быстрое переключение пользователя (для тестов):
                </div>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
                  {demoUsers.map((u: User) => (
                    <button
                      key={u.id}
                      onClick={() => switchUser(u.id)}
                      className={`p-2.5 rounded-lg text-xs font-semibold flex items-center space-x-2 border transition ${
                        user?.id === u.id
                          ? 'bg-discord-blurple text-white border-transparent'
                          : 'bg-discord-dark text-discord-text-muted border-discord-hover hover:text-white'
                      }`}
                    >
                      <img src={u.avatar} alt={u.username} className="w-6 h-6 rounded-full bg-discord-sidebar" />
                      <span className="truncate">{u.username}</span>
                    </button>
                  ))}
                </div>
              </div>
            </div>
          )}

          <div className="pt-6 flex justify-end">
            <button
              onClick={onClose}
              className="bg-discord-blurple hover:bg-discord-blurple-hover text-white px-5 py-2 rounded-lg font-semibold text-sm transition"
            >
              Готово
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
