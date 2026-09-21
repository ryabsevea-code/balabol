import { useState, useEffect } from 'react';
import { Server, Channel, User } from './types';
import { useAuth } from './context/AuthContext';
import { ServerList } from './components/Sidebar/ServerList';
import { ChannelList } from './components/Sidebar/ChannelList';
import { MemberList } from './components/Sidebar/MemberList';
import { ChatArea } from './components/Chat/ChatArea';
import { VoiceGrid } from './components/Voice/VoiceGrid';
import { DirectMessagesView } from './components/DM/DirectMessagesView';
import { SettingsModal } from './components/Modals/SettingsModal';
import { CreateChannelModal } from './components/Modals/CreateChannelModal';
import { AuthModal } from './components/Auth/AuthModal';
import { API_BASE } from './utils/config';

export function App() {
  const { user, isLoading } = useAuth();
  const [servers, setServers] = useState<Server[]>([]);
  const [activeServerId, setActiveServerId] = useState<string | null>(null);
  const [activeChannel, setActiveChannel] = useState<Channel | null>(null);
  const [isDMsActive, setIsDMsActive] = useState(false);
  const [showMembers, setShowMembers] = useState(true);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isCreateChannelOpen, setIsCreateChannelOpen] = useState(false);
  const [allUsers, setAllUsers] = useState<User[]>([]);
  const [dmTargetUser, setDmTargetUser] = useState<User | null>(null);

  const fetchServers = async () => {
    try {
      const res = await fetch(`${API_BASE}/servers`);
      const data = await res.json();
      setServers(data.servers || []);
      if (data.servers && data.servers.length > 0 && !activeServerId) {
        const firstServer = data.servers[0];
        setActiveServerId(firstServer.id);
        const firstTextChannel = firstServer.channels.find((c: Channel) => c.type === 'text') || firstServer.channels[0];
        if (firstTextChannel) {
          setActiveChannel(firstTextChannel);
        }
      }
    } catch (err) {
      console.error('Failed to load servers', err);
    }
  };

  const fetchUsers = async () => {
    try {
      const res = await fetch(`${API_BASE}/auth/demo-users`);
      const data = await res.json();
      setAllUsers(data.users || []);
    } catch (err) {
      console.error('Failed to load users', err);
    }
  };

  useEffect(() => {
    fetchServers();
    fetchUsers();
  }, []);

  const activeServer = servers.find(s => s.id === activeServerId) || null;

  const handleSelectServer = (serverId: string) => {
    setActiveServerId(serverId);
    setIsDMsActive(false);
    const s = servers.find(srv => srv.id === serverId);
    if (s && s.channels.length > 0) {
      const textChannel = s.channels.find(c => c.type === 'text') || s.channels[0];
      setActiveChannel(textChannel);
    }
  };

  const handleSelectDMs = () => {
    setIsDMsActive(true);
  };

  const handleSelectChannel = (channel: Channel) => {
    setActiveChannel(channel);
  };

  const handleSelectUserForDM = (targetUser: User) => {
    setDmTargetUser(targetUser);
    setIsDMsActive(true);
  };

  if (isLoading) {
    return (
      <div className="h-screen w-screen bg-discord-dark flex items-center justify-center text-white font-bold">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-discord-blurple mr-3" />
        Загрузка Balabol...
      </div>
    );
  }

  return (
    <div className="h-screen w-screen flex bg-discord-dark overflow-hidden select-none">
      {/* Auth Screen Modal if not logged in */}
      {!user && <AuthModal />}

      {/* 1. Leftmost Server Icons Sidebar */}
      <ServerList
        servers={servers}
        activeServerId={activeServerId}
        isDMsActive={isDMsActive}
        onSelectServer={handleSelectServer}
        onSelectDMs={handleSelectDMs}
        onCreateServer={() => setIsCreateChannelOpen(true)}
      />

      {/* 2. Main Content View */}
      {isDMsActive ? (
        <DirectMessagesView
          initialTargetUser={dmTargetUser}
          onOpenSettings={() => setIsSettingsOpen(true)}
        />
      ) : (
        <div className="flex-1 flex h-full overflow-hidden">
          {/* Channels Sidebar */}
          <ChannelList
            server={activeServer}
            activeChannelId={activeChannel?.id || null}
            onSelectChannel={handleSelectChannel}
            onCreateChannel={() => setIsCreateChannelOpen(true)}
            onOpenSettings={() => setIsSettingsOpen(true)}
          />

          {/* Active Channel View: Text Chat OR Voice Room */}
          {activeChannel?.type === 'voice' ? (
            <VoiceGrid channel={activeChannel} />
          ) : activeChannel ? (
            <ChatArea
              channel={activeChannel}
              onToggleMembers={() => setShowMembers(prev => !prev)}
              showMembers={showMembers}
            />
          ) : (
            <div className="flex-1 bg-discord-chat flex items-center justify-center text-discord-text-muted">
              Выберите канал слева
            </div>
          )}

          {/* Members Sidebar (for text channels) */}
          {activeChannel?.type === 'text' && showMembers && (
            <MemberList
              members={allUsers}
              onSelectUserForDM={handleSelectUserForDM}
            />
          )}
        </div>
      )}

      {/* Settings Modal */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
      />

      {/* Create Channel Modal */}
      {activeServerId && (
        <CreateChannelModal
          isOpen={isCreateChannelOpen}
          serverId={activeServerId}
          onClose={() => setIsCreateChannelOpen(false)}
          onChannelCreated={fetchServers}
        />
      )}
    </div>
  );
}

export default App;
