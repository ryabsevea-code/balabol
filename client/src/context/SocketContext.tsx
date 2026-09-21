import React, { createContext, useContext, useEffect, useState, useRef } from 'react';
import { io, Socket } from 'socket.io-client';
import { VoiceParticipant } from '../types';
import { useAuth } from './AuthContext';
import { VoiceEngine, VoiceEngineConfig } from '../services/voiceEngine';
import { playJoinVoice, playLeaveVoice, playMuteSound } from '../utils/soundEffects';
import { SERVER_URL } from '../utils/config';

interface SocketContextType {
  socket: Socket | null;
  isConnected: boolean;
  globalVoiceState: Record<string, VoiceParticipant[]>;
  activeVoiceChannelId: string | null;
  voiceParticipants: VoiceParticipant[];
  speakingUsers: Record<string, boolean>;
  isMuted: boolean;
  isDeafened: boolean;
  voiceConfig: VoiceEngineConfig;
  joinVoiceChannel: (channelId: string) => Promise<void>;
  leaveVoiceChannel: () => void;
  toggleMute: () => void;
  toggleDeafen: () => void;
  setUserVolume: (userId: string, volume: number) => void;
  getUserVolume: (userId: string) => number;
  updateVoiceConfig: (config: Partial<VoiceEngineConfig>) => void;
}

const SocketContext = createContext<SocketContextType | undefined>(undefined);

export const SocketProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { user } = useAuth();
  const [socket, setSocket] = useState<Socket | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [globalVoiceState, setGlobalVoiceState] = useState<Record<string, VoiceParticipant[]>>({});
  const [activeVoiceChannelId, setActiveVoiceChannelId] = useState<string | null>(null);
  const [voiceParticipants, setVoiceParticipants] = useState<VoiceParticipant[]>([]);
  const [speakingUsers, setSpeakingUsers] = useState<Record<string, boolean>>({});
  const [isMuted, setIsMuted] = useState(false);
  const [isDeafened, setIsDeafened] = useState(false);
  const [, setVolumeTick] = useState(0); // Trigger re-render on volume updates

  const voiceEngineRef = useRef<VoiceEngine | null>(null);
  const [voiceConfig, setVoiceConfig] = useState<VoiceEngineConfig>({
    noiseSuppression: true,
    echoCancellation: true,
    autoGainControl: true,
    sensitivityThreshold: 25,
  });

  // Connect socket
  useEffect(() => {
    const s = io(SERVER_URL, {
      transports: ['websocket', 'polling'],
    });

    s.on('connect', () => {
      setIsConnected(true);
      if (user) {
        s.emit('auth:identify', { userId: user.id });
      }
    });

    s.on('disconnect', () => {
      setIsConnected(false);
    });

    s.on('voice:global-state', (state: Record<string, VoiceParticipant[]>) => {
      setGlobalVoiceState(state);
    });

    // Initialize VoiceEngine
    const engine = new VoiceEngine(s);
    engine.setCallbacks(
      (userId, isSpeaking) => {
        setSpeakingUsers(prev => ({ ...prev, [userId]: isSpeaking }));
      },
      (participants) => {
        setVoiceParticipants(participants);
      }
    );
    voiceEngineRef.current = engine;
    setSocket(s);

    return () => {
      engine.leaveChannel();
      s.disconnect();
    };
  }, []);

  // Update identification if user changes
  useEffect(() => {
    if (socket && isConnected && user) {
      socket.emit('auth:identify', { userId: user.id });
    }
  }, [user, socket, isConnected]);

  const joinVoiceChannel = async (channelId: string) => {
    if (!voiceEngineRef.current || !user) return;

    if (activeVoiceChannelId === channelId) return;

    try {
      if (activeVoiceChannelId) {
        voiceEngineRef.current.leaveChannel();
      }

      await voiceEngineRef.current.joinChannel(channelId, user.id);
      setActiveVoiceChannelId(channelId);
      playJoinVoice();
    } catch (err) {
      console.error('Failed to join voice channel', err);
    }
  };

  const leaveVoiceChannel = () => {
    if (!voiceEngineRef.current) return;
    voiceEngineRef.current.leaveChannel();
    setActiveVoiceChannelId(null);
    setVoiceParticipants([]);
    setSpeakingUsers({});
    playLeaveVoice();
  };

  const toggleMute = () => {
    if (!voiceEngineRef.current) return;
    const newMuted = !isMuted;
    setIsMuted(newMuted);
    voiceEngineRef.current.setMute(newMuted);
    playMuteSound(newMuted);
  };

  const toggleDeafen = () => {
    if (!voiceEngineRef.current) return;
    const newDeafened = !isDeafened;
    setIsDeafened(newDeafened);
    voiceEngineRef.current.setDeafen(newDeafened);
    if (newDeafened) {
      setIsMuted(true);
    }
    playMuteSound(newDeafened);
  };

  const setUserVolume = (userId: string, volume: number) => {
    if (!voiceEngineRef.current) return;
    voiceEngineRef.current.setUserVolume(userId, volume);
    setVolumeTick(t => t + 1);
  };

  const getUserVolume = (userId: string): number => {
    if (!voiceEngineRef.current) return 1.0;
    return voiceEngineRef.current.getUserVolume(userId);
  };

  const updateVoiceConfig = (newConfig: Partial<VoiceEngineConfig>) => {
    setVoiceConfig(prev => {
      const updated = { ...prev, ...newConfig };
      voiceEngineRef.current?.updateConfig(updated);
      return updated;
    });
  };

  return (
    <SocketContext.Provider
      value={{
        socket,
        isConnected,
        globalVoiceState,
        activeVoiceChannelId,
        voiceParticipants,
        speakingUsers,
        isMuted,
        isDeafened,
        voiceConfig,
        joinVoiceChannel,
        leaveVoiceChannel,
        toggleMute,
        toggleDeafen,
        setUserVolume,
        getUserVolume,
        updateVoiceConfig,
      }}
    >
      {children}
    </SocketContext.Provider>
  );
};

export const useSocket = () => {
  const ctx = useContext(SocketContext);
  if (!ctx) throw new Error('useSocket must be used within a SocketProvider');
  return ctx;
};
