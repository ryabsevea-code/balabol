import React from 'react';
import { User } from '../../types';

interface MemberListProps {
  members: User[];
  onSelectUserForDM: (user: User) => void;
}

export const MemberList: React.FC<MemberListProps> = ({ members, onSelectUserForDM }) => {
  const onlineMembers = members.filter(m => m.status !== 'offline');
  const offlineMembers = members.filter(m => m.status === 'offline');

  const getStatusColor = (status: User['status']) => {
    switch (status) {
      case 'online': return 'bg-discord-green';
      case 'idle': return 'bg-discord-yellow';
      case 'dnd': return 'bg-discord-red';
      default: return 'bg-gray-500';
    }
  };

  const renderMember = (member: User) => (
    <div
      key={member.id}
      onClick={() => onSelectUserForDM(member)}
      className="flex items-center px-2 py-1.5 rounded hover:bg-discord-hover cursor-pointer transition group"
      title={`Написать личное сообщение ${member.username}`}
    >
      <div className="relative mr-3 flex-shrink-0">
        <img
          src={member.avatar}
          alt={member.username}
          className="w-8 h-8 rounded-full bg-discord-dark object-cover"
        />
        <div
          className={`absolute bottom-0 right-0 w-2.5 h-2.5 rounded-full ring-2 ring-discord-sidebar ${getStatusColor(
            member.status
          )}`}
        />
      </div>

      <div className="truncate min-w-0">
        <div className="text-sm font-semibold text-discord-text-normal group-hover:text-white truncate">
          {member.username}
        </div>
        {member.customStatus && (
          <div className="text-[11px] text-discord-text-muted truncate">
            {member.customStatus}
          </div>
        )}
      </div>
    </div>
  );

  return (
    <aside aria-label="Список участников" className="w-60 bg-discord-sidebar flex flex-col h-full border-l border-discord-dark flex-shrink-0 select-none overflow-y-auto p-3">
      {/* Online */}
      {onlineMembers.length > 0 && (
        <div className="mb-4">
          <div className="text-[11px] font-bold text-discord-text-muted px-2 mb-1 tracking-wider uppercase">
            В сети — {onlineMembers.length}
          </div>
          <div className="space-y-0.5">
            {onlineMembers.map(renderMember)}
          </div>
        </div>
      )}

      {/* Offline */}
      {offlineMembers.length > 0 && (
        <div>
          <div className="text-[11px] font-bold text-discord-text-muted px-2 mb-1 tracking-wider uppercase">
            Не в сети — {offlineMembers.length}
          </div>
          <div className="space-y-0.5 opacity-60">
            {offlineMembers.map(renderMember)}
          </div>
        </div>
      )}
    </aside>
  );
};
