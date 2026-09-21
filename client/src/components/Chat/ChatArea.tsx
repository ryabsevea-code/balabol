import React, { useState, useEffect, useRef } from 'react';
import {
  Hash,
  Send,
  Smile,
  Paperclip,
  Bell,
  Pin,
  Users,
  Search,
} from 'lucide-react';
import { Channel, Message } from '../../types';
import { useAuth } from '../../context/AuthContext';
import { useSocket } from '../../context/SocketContext';
import { playMessageSound } from '../../utils/soundEffects';
import { API_BASE } from '../../utils/config';

interface ChatAreaProps {
  channel: Channel;
  onToggleMembers: () => void;
  showMembers: boolean;
}

export const ChatArea: React.FC<ChatAreaProps> = ({
  channel,
  onToggleMembers,
  showMembers,
}) => {
  const { user } = useAuth();
  const { socket } = useSocket();
  const [messages, setMessages] = useState<Message[]>([]);
  const [inputText, setInputText] = useState('');
  const [typingUsers, setTypingUsers] = useState<string[]>([]);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const typingTimeoutRef = useRef<number | null>(null);

  // Fetch messages history
  useEffect(() => {
    fetch(`${API_BASE}/channels/${channel.id}/messages`)
      .then(res => res.json())
      .then(data => {
        setMessages(data.messages || []);
      })
      .catch(err => console.error('Failed to load messages', err));
  }, [channel.id]);

  // Socket listeners for channel
  useEffect(() => {
    if (!socket) return;

    socket.emit('chat:join-channel', { channelId: channel.id });

    const handleNewMessage = (msg: Message) => {
      if (msg.channelId === channel.id) {
        setMessages(prev => [...prev, msg]);
        if (msg.authorId !== user?.id) {
          playMessageSound();
        }
      }
    };

    const handleTyping = ({ channelId, username, isTyping }: { channelId: string; username: string; isTyping: boolean }) => {
      if (channelId !== channel.id) return;

      setTypingUsers(prev => {
        if (isTyping && !prev.includes(username)) {
          return [...prev, username];
        } else if (!isTyping) {
          return prev.filter(u => u !== username);
        }
        return prev;
      });
    };

    socket.on('chat:new-message', handleNewMessage);
    socket.on('chat:user-typing', handleTyping);

    return () => {
      socket.emit('chat:leave-channel', { channelId: channel.id });
      socket.off('chat:new-message', handleNewMessage);
      socket.off('chat:user-typing', handleTyping);
    };
  }, [socket, channel.id, user?.id]);

  // Auto scroll
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setInputText(e.target.value);

    if (!socket || !user) return;

    // Send typing event
    socket.emit('chat:typing', {
      channelId: channel.id,
      username: user.username,
      isTyping: true,
    });

    if (typingTimeoutRef.current) clearTimeout(typingTimeoutRef.current);
    typingTimeoutRef.current = window.setTimeout(() => {
      socket.emit('chat:typing', {
        channelId: channel.id,
        username: user.username,
        isTyping: false,
      });
    }, 2000);
  };

  const handleSendMessage = (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    if (!inputText.trim() || !socket || !user) return;

    socket.emit('chat:send-message', {
      channelId: channel.id,
      content: inputText,
      authorId: user.id,
    });

    if (typingTimeoutRef.current) clearTimeout(typingTimeoutRef.current);
    socket.emit('chat:typing', {
      channelId: channel.id,
      username: user.username,
      isTyping: false,
    });

    setInputText('');
  };

  const formatTime = (isoString: string) => {
    try {
      const date = new Date(isoString);
      return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch {
      return '';
    }
  };

  return (
    <div className="flex-1 bg-discord-chat flex flex-col h-full overflow-hidden">
      {/* Channel Header */}
      <div className="h-12 border-b border-discord-dark px-4 flex items-center justify-between shadow-sm bg-discord-chat z-10">
        <div className="flex items-center space-x-2 truncate">
          <Hash size={24} className="text-discord-text-muted" />
          <h1 className="font-bold text-white text-base truncate">{channel.name}</h1>
          {channel.description && (
            <>
              <div className="w-[1px] h-4 bg-discord-hover mx-2" />
              <span className="text-xs text-discord-text-muted truncate">
                {channel.description}
              </span>
            </>
          )}
        </div>

        {/* Right Header Actions */}
        <div className="flex items-center space-x-3 text-discord-text-muted">
          <button className="hover:text-white transition p-1" title="Уведомления">
            <Bell size={20} />
          </button>
          <button className="hover:text-white transition p-1" title="Закрепленные сообщения">
            <Pin size={20} />
          </button>
          <button
            onClick={onToggleMembers}
            className={`transition p-1 ${showMembers ? 'text-white' : 'hover:text-white'}`}
            title="Список участников"
          >
            <Users size={20} />
          </button>
          <div className="relative flex items-center">
            <input
              type="text"
              placeholder="Поиск..."
              className="bg-discord-dark text-xs text-discord-text-normal pl-2 pr-6 py-1 rounded w-36 focus:w-48 transition-all duration-200 outline-none placeholder-discord-text-muted"
            />
            <Search size={14} className="absolute right-2 text-discord-text-muted pointer-events-none" />
          </div>
        </div>
      </div>

      {/* Messages Scroll Area */}
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
        {/* Welcome message */}
        <div className="mb-6 pt-4">
          <div className="w-16 h-16 bg-discord-sidebar rounded-full flex items-center justify-center mb-2">
            <Hash size={36} className="text-white" />
          </div>
          <h2 className="text-2xl font-extrabold text-white">Добро пожаловать в #{channel.name}!</h2>
          <p className="text-sm text-discord-text-muted mt-1">
            Это начало канала #{channel.name}.
          </p>
        </div>

        {/* Messages List */}
        {messages.map((msg, index) => {
          const prevMsg = messages[index - 1];
          const isSameAuthor = prevMsg && prevMsg.authorId === msg.authorId;

          return (
            <div
              key={msg.id}
              className={`flex items-start group hover:bg-[#1a3023] -mx-4 px-4 py-1.5 rounded transition ${
                !isSameAuthor ? 'pt-2' : ''
              }`}
            >
              {!isSameAuthor ? (
                <img
                  src={msg.author.avatar || 'https://api.dicebear.com/7.x/bottts/svg?seed=User'}
                  alt={msg.author.username}
                  className="w-10 h-10 rounded-full bg-discord-dark mr-4 mt-0.5 object-cover flex-shrink-0"
                />
              ) : (
                <div className="w-10 mr-4 text-[10px] text-discord-text-muted opacity-0 group-hover:opacity-100 text-right select-none pt-0.5">
                  {formatTime(msg.createdAt)}
                </div>
              )}

              <div className="flex-1 min-w-0">
                {!isSameAuthor && (
                  <div className="flex items-baseline space-x-2">
                    <span className="font-semibold text-white text-sm hover:underline cursor-pointer">
                      {msg.author.username}
                    </span>
                    <span className="text-[11px] text-discord-text-muted">
                      {formatTime(msg.createdAt)}
                    </span>
                  </div>
                )}
                <div className="text-sm text-discord-text-normal leading-relaxed break-words">
                  {msg.content}
                </div>
              </div>
            </div>
          );
        })}
        <div ref={messagesEndRef} />
      </div>

      {/* Typing Indicator */}
      <div className="h-6 px-4 text-xs text-discord-text-muted flex items-center">
        {typingUsers.length > 0 && (
          <span className="animate-pulse">
            <strong className="text-white">{typingUsers.join(', ')}</strong>{' '}
            {typingUsers.length === 1 ? 'печатает...' : 'печатают...'}
          </span>
        )}
      </div>

      {/* Message Input Box */}
      <div className="px-4 pb-4">
        <form
          onSubmit={handleSendMessage}
          className="bg-discord-input rounded-lg flex items-center px-4 py-2.5 space-x-3 shadow-inner"
        >
          <button
            type="button"
            className="text-discord-text-muted hover:text-white transition p-1 bg-discord-hover rounded-full"
            title="Прикрепить файл"
          >
            <Paperclip size={18} />
          </button>

          <input
            type="text"
            value={inputText}
            onChange={handleInputChange}
            placeholder={`Написать в #${channel.name}...`}
            className="flex-1 bg-transparent text-sm text-white placeholder-discord-text-muted outline-none"
          />

          <button
            type="button"
            className="text-discord-text-muted hover:text-white transition p-1"
            title="Эмодзи"
          >
            <Smile size={20} />
          </button>

          <button
            type="submit"
            disabled={!inputText.trim()}
            className={`p-1.5 rounded transition ${
              inputText.trim()
                ? 'bg-discord-blurple text-white hover:bg-discord-blurple-hover shadow'
                : 'text-discord-text-muted opacity-40 cursor-not-allowed'
            }`}
            title="Отправить сообщение"
          >
            <Send size={16} />
          </button>
        </form>
      </div>
    </div>
  );
};
