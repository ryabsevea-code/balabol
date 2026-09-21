import React, { useState, useEffect, useRef } from 'react';
import {
  Users,
  Phone,
  PhoneOff,
  Send,
  Smile,
  Paperclip,
  AtSign,
} from 'lucide-react';
import { User, DMConversation } from '../../types';
import { useAuth } from '../../context/AuthContext';
import { useSocket } from '../../context/SocketContext';
import { playMessageSound } from '../../utils/soundEffects';
import { API_BASE } from '../../utils/config';

interface DirectMessagesViewProps {
  initialTargetUser?: User | null;
  onOpenSettings: () => void;
}

export const DirectMessagesView: React.FC<DirectMessagesViewProps> = ({ initialTargetUser }) => {
  const { user, demoUsers } = useAuth();
  const { socket, joinVoiceChannel, leaveVoiceChannel, activeVoiceChannelId } = useSocket();
  const [dms, setDms] = useState<DMConversation[]>([]);
  const [activeDM, setActiveDM] = useState<DMConversation | null>(null);
  const [messages, setMessages] = useState<any[]>([]);
  const [inputText, setInputText] = useState('');
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Load user DMs
  const loadDMs = async () => {
    if (!user) return;
    try {
      const res = await fetch(`${API_BASE}/dms/${user.id}`);
      const data = await res.json();
      setDms(data.dms || []);

      if (initialTargetUser) {
        startDMWith(initialTargetUser);
      } else if (data.dms && data.dms.length > 0 && !activeDM) {
        setActiveDM(data.dms[0]);
      }
    } catch (err) {
      console.error('Failed to load DMs', err);
    }
  };

  useEffect(() => {
    loadDMs();
  }, [user, initialTargetUser]);

  const startDMWith = async (targetUser: User) => {
    if (!user || user.id === targetUser.id) return;
    try {
      const res = await fetch(`${API_BASE}/dms`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ userA: user.id, userB: targetUser.id }),
      });
      const data = await res.json();
      setActiveDM(data.dm);
      loadDMs();
    } catch (err) {
      console.error('Failed to start DM', err);
    }
  };

  // Load DM messages
  useEffect(() => {
    if (!activeDM) return;

    fetch(`${API_BASE}/dms/${activeDM.id}/messages`)
      .then(res => res.json())
      .then(data => {
        setMessages(data.messages || []);
      })
      .catch(err => console.error('Failed to load DM messages', err));
  }, [activeDM?.id]);

  // Socket listener for DMs
  useEffect(() => {
    if (!socket || !activeDM) return;

    socket.emit('dm:join', { dmId: activeDM.id });

    const handleNewDMMessage = (msg: any) => {
      if (msg.dmId === activeDM.id) {
        setMessages(prev => [...prev, msg]);
        if (msg.authorId !== user?.id) {
          playMessageSound();
        }
      }
    };

    socket.on('dm:new-message', handleNewDMMessage);

    return () => {
      socket.off('dm:new-message', handleNewDMMessage);
    };
  }, [socket, activeDM?.id, user?.id]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleSendMessage = (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputText.trim() || !socket || !user || !activeDM) return;

    socket.emit('dm:send-message', {
      dmId: activeDM.id,
      content: inputText,
      authorId: user.id,
    });

    setInputText('');
  };

  // Start 1-on-1 voice call using a designated room
  const dmVoiceChannelId = activeDM ? `dm_call_${activeDM.id}` : null;
  const isCallActive = activeVoiceChannelId === dmVoiceChannelId;

  const handleToggleCall = () => {
    if (!dmVoiceChannelId) return;
    if (isCallActive) {
      leaveVoiceChannel();
    } else {
      joinVoiceChannel(dmVoiceChannelId);
    }
  };

  const otherUser = activeDM?.otherUser;

  return (
    <div className="flex-1 flex h-full overflow-hidden bg-discord-chat">
      {/* DM Sidebar */}
      <div className="w-60 bg-discord-sidebar flex flex-col h-full border-r border-discord-dark flex-shrink-0">
        {/* Friends Button */}
        <div className="p-3 border-b border-discord-dark">
          <button className="w-full flex items-center space-x-3 px-3 py-2 rounded bg-discord-active text-white text-sm font-semibold">
            <Users size={18} />
            <span>Друзья</span>
          </button>
        </div>

        {/* DM List Header */}
        <div className="flex-1 overflow-y-auto p-2 space-y-1">
          <div className="text-[11px] font-bold text-discord-text-muted px-2 py-1 tracking-wider uppercase">
            Личные сообщения
          </div>

          {/* Quick list of other demo users to chat with */}
          {demoUsers
            .filter((u: User) => u.id !== user?.id)
            .map((target: User) => {
              const isSelected = activeDM?.otherUser?.id === target.id;
              return (
                <button
                  key={target.id}
                  onClick={() => startDMWith(target)}
                  className={`w-full flex items-center space-x-2.5 px-2 py-2 rounded text-sm transition ${
                    isSelected
                      ? 'bg-discord-active text-white'
                      : 'text-discord-text-muted hover:bg-discord-hover hover:text-white'
                  }`}
                >
                  <div className="relative flex-shrink-0">
                    <img src={target.avatar} alt={target.username} className="w-7 h-7 rounded-full bg-discord-dark" />
                    <div className="absolute bottom-0 right-0 w-2 h-2 rounded-full bg-discord-green ring-2 ring-discord-sidebar" />
                  </div>
                  <div className="truncate text-left">
                    <div className="font-semibold text-xs truncate">{target.username}</div>
                    <div className="text-[10px] text-discord-text-muted truncate">
                      {target.customStatus || 'Нажмите для чата'}
                    </div>
                  </div>
                </button>
              );
            })}
        </div>
      </div>

      {/* DM Main Chat Area */}
      {activeDM && otherUser ? (
        <div className="flex-1 flex flex-col h-full overflow-hidden">
          {/* DM Header */}
          <div className="h-12 border-b border-discord-dark px-4 flex items-center justify-between shadow-sm bg-discord-chat">
            <div className="flex items-center space-x-2 truncate">
              <AtSign size={20} className="text-discord-text-muted" />
              <span className="font-bold text-white text-base truncate">{otherUser.username}</span>
              <div className="w-2 h-2 rounded-full bg-discord-green ml-1" />
            </div>

            {/* Direct Voice Call Button */}
            <div className="flex items-center space-x-2">
              <button
                onClick={handleToggleCall}
                className={`flex items-center space-x-1.5 px-3 py-1.5 rounded text-xs font-bold transition shadow ${
                  isCallActive
                    ? 'bg-discord-red hover:bg-red-600 text-white animate-pulse'
                    : 'bg-discord-green hover:bg-green-600 text-white'
                }`}
                title={isCallActive ? 'Завершить звонок' : 'Начать голосовой звонок'}
              >
                {isCallActive ? <PhoneOff size={14} /> : <Phone size={14} />}
                <span>{isCallActive ? 'Завершить звонок' : 'Голосовой звонок 1-на-1'}</span>
              </button>
            </div>
          </div>

          {/* DM Messages */}
          <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
            <div className="mb-6 pt-4 text-center">
              <img
                src={otherUser.avatar}
                alt={otherUser.username}
                className="w-20 h-20 rounded-full mx-auto mb-2 bg-discord-dark"
              />
              <h2 className="text-xl font-bold text-white">{otherUser.username}</h2>
              <p className="text-xs text-discord-text-muted mt-1">
                Это начало вашей личной переписки и звонков с {otherUser.username}.
              </p>
            </div>

            {messages.map((msg, index) => (
              <div key={msg.id || index} className="flex items-start hover:bg-[#1a3023] -mx-4 px-4 py-1.5 rounded transition">
                <img
                  src={msg.author?.avatar || 'https://api.dicebear.com/7.x/bottts/svg?seed=User'}
                  alt="avatar"
                  className="w-10 h-10 rounded-full mr-3 bg-discord-dark flex-shrink-0"
                />
                <div>
                  <div className="flex items-baseline space-x-2">
                    <span className="font-semibold text-white text-sm">{msg.author?.username}</span>
                    <span className="text-[10px] text-discord-text-muted">
                      {new Date(msg.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                    </span>
                  </div>
                  <div className="text-sm text-discord-text-normal mt-0.5">{msg.content}</div>
                </div>
              </div>
            ))}
            <div ref={messagesEndRef} />
          </div>

          {/* DM Input */}
          <div className="px-4 pb-4">
            <form
              onSubmit={handleSendMessage}
              className="bg-discord-input rounded-lg flex items-center px-4 py-2.5 space-x-3 shadow-inner"
            >
              <button
                type="button"
                className="text-discord-text-muted hover:text-white transition p-1 bg-discord-hover rounded-full"
              >
                <Paperclip size={18} />
              </button>
              <input
                type="text"
                value={inputText}
                onChange={e => setInputText(e.target.value)}
                placeholder={`Написать @${otherUser.username}...`}
                className="flex-1 bg-transparent text-sm text-white placeholder-discord-text-muted outline-none"
              />
              <button type="button" className="text-discord-text-muted hover:text-white transition p-1">
                <Smile size={20} />
              </button>
              <button
                type="submit"
                disabled={!inputText.trim()}
                className="p-1.5 rounded bg-discord-blurple text-white hover:bg-discord-blurple-hover transition disabled:opacity-40"
              >
                <Send size={16} />
              </button>
            </form>
          </div>
        </div>
      ) : (
        <div className="flex-1 flex flex-col items-center justify-center p-8 text-center text-discord-text-muted">
          <Users size={48} className="mb-3 opacity-40" />
          <h3 className="text-lg font-bold text-white mb-1">Личные сообщения</h3>
          <p className="text-sm max-w-sm">
            Выберите друга слева, чтобы начать личную переписку или созвониться напрямую без серверов.
          </p>
        </div>
      )}
    </div>
  );
};
