import { Socket } from 'socket.io-client';
import { VoiceParticipant } from '../types';

export interface VoiceEngineConfig {
  noiseSuppression: boolean;
  echoCancellation: boolean;
  autoGainControl: boolean;
  sensitivityThreshold: number; // 0 to 100
}

type SpeakingChangeCallback = (userId: string, isSpeaking: boolean) => void;
type ParticipantsChangeCallback = (participants: VoiceParticipant[]) => void;

const ICE_SERVERS: RTCConfiguration = {
  iceServers: [
    { urls: 'stun:stun.l.google.com:19302' },
    { urls: 'stun:stun1.l.google.com:19302' },
    { urls: 'stun:stun2.l.google.com:19302' },
  ],
};

export class VoiceEngine {
  private socket: Socket;
  private currentChannelId: string | null = null;
  private currentUserId: string | null = null;
  private localStream: MediaStream | null = null;
  private peerConnections = new Map<string, RTCPeerConnection>(); // socketId -> RTCPeerConnection
  private peerAudioElements = new Map<string, HTMLAudioElement>(); // socketId -> audio element
  private gainNodes = new Map<string, GainNode>(); // userId -> GainNode
  private userVolumes = new Map<string, number>(); // userId -> volume (0.0 to 2.0)
  
  private audioContext: AudioContext | null = null;
  private localAnalyser: AnalyserNode | null = null;
  private vadInterval: number | null = null;

  public config: VoiceEngineConfig = {
    noiseSuppression: true,
    echoCancellation: true,
    autoGainControl: true,
    sensitivityThreshold: 25,
  };

  public isMuted = false;
  public isDeafened = false;
  private isSpeaking = false;

  private onSpeakingChange: SpeakingChangeCallback | null = null;
  private onParticipantsChange: ParticipantsChangeCallback | null = null;
  private participants: VoiceParticipant[] = [];

  constructor(socket: Socket) {
    this.socket = socket;
    this.loadSavedVolumes();
    this.setupSocketListeners();
  }

  private loadSavedVolumes() {
    try {
      const saved = localStorage.getItem('balabol_user_volumes');
      if (saved) {
        const parsed = JSON.parse(saved);
        Object.entries(parsed).forEach(([userId, vol]) => {
          this.userVolumes.set(userId, Number(vol));
        });
      }
    } catch (e) {
      console.error('Failed to load user volumes', e);
    }
  }

  private saveUserVolumes() {
    try {
      const obj: Record<string, number> = {};
      this.userVolumes.forEach((v, k) => { obj[k] = v; });
      localStorage.setItem('balabol_user_volumes', JSON.stringify(obj));
    } catch (e) {
      console.error('Failed to save user volumes', e);
    }
  }

  public setCallbacks(onSpeaking: SpeakingChangeCallback, onParticipants: ParticipantsChangeCallback) {
    this.onSpeakingChange = onSpeaking;
    this.onParticipantsChange = onParticipants;
  }

  private getAudioContext(): AudioContext {
    if (!this.audioContext) {
      const AudioCtx = window.AudioContext || (window as any).webkitAudioContext;
      this.audioContext = new AudioCtx();
    }
    if (this.audioContext.state === 'suspended') {
      this.audioContext.resume();
    }
    return this.audioContext;
  }

  // Set individual volume for a specific participant (0% to 200%)
  public setUserVolume(userId: string, volume: number) {
    this.userVolumes.set(userId, volume);
    this.saveUserVolumes();

    const gainNode = this.gainNodes.get(userId);
    if (gainNode) {
      gainNode.gain.setValueAtTime(this.isDeafened ? 0 : volume, this.audioContext?.currentTime || 0);
    }
  }

  public getUserVolume(userId: string): number {
    return this.userVolumes.get(userId) ?? 1.0;
  }

  // Join a voice channel
  public async joinChannel(channelId: string, userId: string) {
    this.currentChannelId = channelId;
    this.currentUserId = userId;

    try {
      await this.initLocalStream();
      this.startVAD();

      // Emit join to server
      this.socket.emit('voice:join', { channelId, userId });
    } catch (err) {
      console.error('Error joining voice channel:', err);
      throw err;
    }
  }

  // Leave current voice channel
  public leaveChannel() {
    if (!this.currentChannelId) return;

    this.socket.emit('voice:leave');

    // Clean up local media
    this.stopVAD();
    if (this.localStream) {
      this.localStream.getTracks().forEach(t => t.stop());
      this.localStream = null;
    }

    // Close all peer connections
    this.peerConnections.forEach(pc => pc.close());
    this.peerConnections.clear();

    // Clean up audio elements
    this.peerAudioElements.forEach(audio => {
      audio.pause();
      audio.srcObject = null;
      audio.remove();
    });
    this.peerAudioElements.clear();
    this.gainNodes.clear();

    this.currentChannelId = null;
    this.participants = [];
    if (this.onParticipantsChange) {
      this.onParticipantsChange([]);
    }
  }

  // Toggle Mute
  public setMute(muted: boolean) {
    this.isMuted = muted;
    if (this.localStream) {
      this.localStream.getAudioTracks().forEach(track => {
        track.enabled = !muted;
      });
    }

    this.socket.emit('voice:state-change', { isMuted: muted });
  }

  // Toggle Deafen
  public setDeafen(deafened: boolean) {
    this.isDeafened = deafened;

    // When deafened, also mute microphone
    if (deafened && !this.isMuted) {
      this.setMute(true);
    }

    // Mute/unmute all incoming audio
    this.gainNodes.forEach((gain, userId) => {
      const vol = deafened ? 0 : this.getUserVolume(userId);
      gain.gain.setValueAtTime(vol, this.audioContext?.currentTime || 0);
    });

    this.socket.emit('voice:state-change', { isDeafened: deafened });
  }

  // Update noise suppression configuration on the fly
  public async updateConfig(newConfig: Partial<VoiceEngineConfig>) {
    this.config = { ...this.config, ...newConfig };
    if (this.currentChannelId && this.localStream) {
      // Re-initialize mic stream with new constraints
      const oldTracks = this.localStream.getTracks();
      await this.initLocalStream();

      // Replace tracks in active peer connections
      const newTrack = this.localStream?.getAudioTracks()[0];
      if (newTrack) {
        this.peerConnections.forEach(pc => {
          const sender = pc.getSenders().find(s => s.track?.kind === 'audio');
          if (sender) {
            sender.replaceTrack(newTrack);
          }
        });
      }

      oldTracks.forEach(t => t.stop());
    }
  }

  // Initialize microphone stream
  private async initLocalStream() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          noiseSuppression: this.config.noiseSuppression,
          echoCancellation: this.config.echoCancellation,
          autoGainControl: this.config.autoGainControl,
        },
        video: false,
      });

      this.localStream = stream;
      if (this.isMuted) {
        stream.getAudioTracks().forEach(t => { t.enabled = false; });
      }

      return stream;
    } catch (err) {
      console.warn('Microphone access failed or denied, using silent mock track', err);
      // Create empty/silent audio track if user has no microphone plugged in
      const ctx = this.getAudioContext();
      const osc = ctx.createOscillator();
      const dst = ctx.createMediaStreamDestination();
      osc.connect(dst);
      osc.start();
      this.localStream = dst.stream;
      return this.localStream;
    }
  }

  // Voice Activity Detection (RMS volume analysis)
  private startVAD() {
    if (!this.localStream) return;

    try {
      const ctx = this.getAudioContext();
      const source = ctx.createMediaStreamSource(this.localStream);
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 256;
      source.connect(analyser);
      this.localAnalyser = analyser;

      const dataArray = new Uint8Array(analyser.frequencyBinCount);

      this.vadInterval = window.setInterval(() => {
        if (this.isMuted || !this.currentChannelId) {
          if (this.isSpeaking) {
            this.isSpeaking = false;
            this.broadcastSpeaking(false);
          }
          return;
        }

        analyser.getByteFrequencyData(dataArray);
        let sum = 0;
        for (let i = 0; i < dataArray.length; i++) {
          sum += dataArray[i];
        }
        const average = sum / dataArray.length;
        const threshold = this.config.sensitivityThreshold;
        const nowSpeaking = average > threshold;

        if (nowSpeaking !== this.isSpeaking) {
          this.isSpeaking = nowSpeaking;
          this.broadcastSpeaking(nowSpeaking);
        }
      }, 80);
    } catch (err) {
      console.error('Failed to setup VAD analyzer', err);
    }
  }

  private stopVAD() {
    if (this.vadInterval) {
      clearInterval(this.vadInterval);
      this.vadInterval = null;
    }
    if (this.isSpeaking) {
      this.isSpeaking = false;
      this.broadcastSpeaking(false);
    }
  }

  private broadcastSpeaking(isSpeaking: boolean) {
    if (this.currentUserId && this.onSpeakingChange) {
      this.onSpeakingChange(this.currentUserId, isSpeaking);
    }
    this.socket.emit('voice:state-change', { isSpeaking });
  }

  // Socket listeners for WebRTC Signaling
  private setupSocketListeners() {
    // 1. Initial participants when joining
    this.socket.on('voice:users-in-room', async ({ users }: { channelId: string; users: VoiceParticipant[] }) => {
      this.participants = users;
      if (this.onParticipantsChange) {
        this.onParticipantsChange(this.participants);
      }

      // Initiate WebRTC connection to each existing user (we are the caller)
      for (const peer of users) {
        await this.createPeerConnection(peer.socketId, peer.userId, true);
      }
    });

    // 2. Another user joined room
    this.socket.on('voice:user-joined', async ({ participant }: { channelId: string; participant: VoiceParticipant }) => {
      if (participant.socketId === this.socket.id) return;

      this.participants = [...this.participants.filter(p => p.socketId !== participant.socketId), participant];
      if (this.onParticipantsChange) {
        this.onParticipantsChange(this.participants);
      }

      // Ready to receive offer from new user
      await this.createPeerConnection(participant.socketId, participant.userId, false);
    });

    // 3. User left room
    this.socket.on('voice:user-left', ({ socketId, userId }: { channelId: string; socketId: string; userId: string }) => {
      this.closePeer(socketId, userId);
      this.participants = this.participants.filter(p => p.socketId !== socketId);
      if (this.onParticipantsChange) {
        this.onParticipantsChange(this.participants);
      }
    });

    // 4. Participant state update (mute, deaf, speaking)
    this.socket.on('voice:user-updated', ({ participant }: { channelId: string; participant: VoiceParticipant }) => {
      this.participants = this.participants.map(p => p.socketId === participant.socketId ? participant : p);
      if (this.onParticipantsChange) {
        this.onParticipantsChange(this.participants);
      }
      if (this.onSpeakingChange) {
        this.onSpeakingChange(participant.userId, participant.isSpeaking);
      }
    });

    // 5. WebRTC signal received
    this.socket.on('voice:signal', async ({ fromSocketId, fromUserId, signalData }: {
      fromSocketId: string;
      fromUserId: string;
      signalData: any;
    }) => {
      let pc = this.peerConnections.get(fromSocketId);
      if (!pc) {
        pc = await this.createPeerConnection(fromSocketId, fromUserId, false);
      }

      try {
        if (signalData.type === 'offer') {
          await pc.setRemoteDescription(new RTCSessionDescription(signalData));
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          this.socket.emit('voice:signal', {
            toSocketId: fromSocketId,
            signalData: answer,
          });
        } else if (signalData.type === 'answer') {
          await pc.setRemoteDescription(new RTCSessionDescription(signalData));
        } else if (signalData.candidate) {
          await pc.addIceCandidate(new RTCIceCandidate(signalData.candidate));
        }
      } catch (err) {
        console.error('Error handling WebRTC signal:', err);
      }
    });
  }

  // Create RTCPeerConnection and setup Web Audio GainNode for volume control
  private async createPeerConnection(remoteSocketId: string, remoteUserId: string, isInitiator: boolean): Promise<RTCPeerConnection> {
    const pc = new RTCPeerConnection(ICE_SERVERS);
    this.peerConnections.set(remoteSocketId, pc);

    // Add local tracks to send to peer
    if (this.localStream) {
      this.localStream.getTracks().forEach(track => {
        pc.addTrack(track, this.localStream!);
      });
    }

    // ICE Candidate generation
    pc.onicecandidate = (event) => {
      if (event.candidate) {
        this.socket.emit('voice:signal', {
          toSocketId: remoteSocketId,
          signalData: { candidate: event.candidate },
        });
      }
    };

    // Incoming remote track: setup Web Audio API routing with individual GainNode!
    pc.ontrack = (event) => {
      const [remoteStream] = event.streams;
      if (!remoteStream) return;

      try {
        const ctx = this.getAudioContext();
        const source = ctx.createMediaStreamSource(remoteStream);
        const gainNode = ctx.createGain();

        // Apply saved or default volume (0.0 to 2.0)
        const userVol = this.getUserVolume(remoteUserId);
        gainNode.gain.setValueAtTime(this.isDeafened ? 0 : userVol, ctx.currentTime);

        source.connect(gainNode);
        gainNode.connect(ctx.destination);

        this.gainNodes.set(remoteUserId, gainNode);
      } catch (err) {
        console.warn('Web Audio routing fallback to HTMLAudioElement', err);
      }

      // Also create an audio element fallback
      let audio = this.peerAudioElements.get(remoteSocketId);
      if (!audio) {
        audio = new Audio();
        audio.autoplay = true;
        audio.srcObject = remoteStream;
        this.peerAudioElements.set(remoteSocketId, audio);
      }
    };

    if (isInitiator) {
      try {
        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);
        this.socket.emit('voice:signal', {
          toSocketId: remoteSocketId,
          signalData: offer,
        });
      } catch (err) {
        console.error('Error creating offer:', err);
      }
    }

    return pc;
  }

  private closePeer(socketId: string, userId: string) {
    const pc = this.peerConnections.get(socketId);
    if (pc) {
      pc.close();
      this.peerConnections.delete(socketId);
    }

    const audio = this.peerAudioElements.get(socketId);
    if (audio) {
      audio.pause();
      audio.srcObject = null;
      audio.remove();
      this.peerAudioElements.delete(socketId);
    }

    this.gainNodes.delete(userId);
  }
}
