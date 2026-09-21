import React from 'react';
import { MessageSquare, Plus, Compass } from 'lucide-react';
import { Server } from '../../types';

interface ServerListProps {
  servers: Server[];
  activeServerId: string | null;
  isDMsActive: boolean;
  onSelectServer: (serverId: string) => void;
  onSelectDMs: () => void;
  onCreateServer: () => void;
}

export const ServerList: React.FC<ServerListProps> = ({
  servers,
  activeServerId,
  isDMsActive,
  onSelectServer,
  onSelectDMs,
  onCreateServer,
}) => {
  return (
    <nav aria-label="Список серверов" className="w-[72px] bg-discord-dark border-r border-discord-border/40 flex flex-col items-center py-3 space-y-2 flex-shrink-0 z-20">
      {/* Direct Messages / Home Icon */}
      <div className="relative group flex items-center justify-center w-full">
        {/* Left Indicator Pill */}
        <div
          className={`absolute left-0 w-1 bg-white rounded-r transition-all duration-200 ${
            isDMsActive ? 'h-10' : 'h-0 group-hover:h-5'
          }`}
        />
        <button
          onClick={onSelectDMs}
          className={`w-12 h-12 rounded-[24px] flex items-center justify-center transition-all duration-200 group-hover:rounded-[16px] ${
            isDMsActive
              ? 'bg-discord-blurple text-white rounded-[16px]'
              : 'bg-discord-chat text-discord-text-normal hover:bg-discord-blurple hover:text-white'
          }`}
          title="Личные сообщения"
        >
          <MessageSquare size={26} />
        </button>
      </div>

      {/* Separator */}
      <div className="w-8 h-[2px] bg-discord-sidebar rounded my-1" />

      {/* Server List */}
      <div className="flex flex-col space-y-2 w-full overflow-y-auto overflow-x-hidden">
        {servers.map(server => {
          const isActive = activeServerId === server.id && !isDMsActive;
          return (
            <div key={server.id} className="relative group flex items-center justify-center w-full">
              {/* Left Indicator Pill */}
              <div
                className={`absolute left-0 w-1 bg-white rounded-r transition-all duration-200 ${
                  isActive ? 'h-10' : 'h-0 group-hover:h-5'
                }`}
              />
              <button
                onClick={() => onSelectServer(server.id)}
                className={`w-12 h-12 rounded-[24px] flex items-center justify-center font-bold text-lg transition-all duration-200 group-hover:rounded-[16px] overflow-hidden ${
                  isActive
                    ? 'bg-discord-blurple text-white rounded-[16px]'
                    : 'bg-discord-chat text-discord-text-normal hover:bg-discord-blurple hover:text-white'
                }`}
                title={server.name}
              >
                {server.icon ? (
                  <img src={server.icon} alt={server.name} className="w-full h-full object-cover" />
                ) : (
                  server.name.substring(0, 2).toUpperCase()
                )}
              </button>
            </div>
          );
        })}
      </div>

      {/* Add Server Button */}
      <div className="relative group flex items-center justify-center w-full pt-1">
        <button
          onClick={onCreateServer}
          className="w-12 h-12 rounded-[24px] bg-discord-chat text-discord-green flex items-center justify-center transition-all duration-200 hover:rounded-[16px] hover:bg-discord-green hover:text-white"
          title="Добавить сервер"
        >
          <Plus size={24} />
        </button>
      </div>

      {/* Explore / Public */}
      <div className="relative group flex items-center justify-center w-full">
        <button
          className="w-12 h-12 rounded-[24px] bg-discord-chat text-discord-text-normal flex items-center justify-center transition-all duration-200 hover:rounded-[16px] hover:bg-discord-green hover:text-white"
          title="Публичные серверы"
        >
          <Compass size={24} />
        </button>
      </div>
    </nav>
  );
};
