<script>
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import {
    Radio,
    Users,
    Copy,
    Check,
    PhoneOff,
    Wifi,
    Globe,
    AlertCircle,
    Server,
    Link,
    RefreshCw,
    ArrowRight,
    Send,
    MessageSquare,
    Key,
    ShieldCheck,
    Mic,
    MicOff,
    Headphones,
    Volume2,
    VolumeX,
    Video,
    VideoOff,
    Monitor,
    MonitorOff,
    LayoutGrid,
    Sliders,
    Maximize2,
    X,
    Bell
  } from 'lucide-svelte';

  let status = { type: 'Disconnected' };
  let peers = [];
  let chatMessages = [];
  let pollInterval = null;
  let videoPollInterval = null;

  // Modals state
  let showConnectModal = false;

  // User info
  let nickname = localStorage.getItem('balabol_nickname') || 'Узел-' + Math.floor(1000 + Math.random() * 9000);
  let roomCodeInput = '';

  // Chat input
  let chatInput = '';
  let chatContainer;

  let copiedItem = '';
  let isSubmitting = false;

  // Voice & Video State
  let voiceStatus = {
    mic_muted: false,
    deafened: false,
    peer_volume: 1.0,
    local_speaking: false,
    remote_speaking: false,
    mic_level: 0.0,
    mic_sensitivity: 0.5,
    mic_gain: 1.0,
    output_device_ok: true,
    input_device_ok: true,
    output_device_name: null,
    input_device_name: null,
    remote_media: {
      mic_muted: false,
      deafened: false,
      video_active: false,
      screen_active: false,
    }
  };

  let remoteVideoFrame = null;
  let lastReceivedFrameId = 0;
  let isCameraOn = false;
  let isScreenOn = false;
  let localStream = null;
  let localVideoEl;
  let canvasEl;
  let videoCaptureTimer = null;
  let peerVolume = parseInt(localStorage.getItem('balabol_peer_vol') || '100');
  let activeTab = 'split'; // 'split' | 'stage' | 'chat'
  let showVolumeSlider = false;

  // Audio Settings & Sensitivity Controls
  let showAudioSettingsModal = false;
  let micSensitivity = parseInt(localStorage.getItem('balabol_mic_sens') || '50');
  let micGain = parseInt(localStorage.getItem('balabol_mic_gain') || '100');
  let isMicTesting = false;
  let audioRestartFeedback = '';

  // Sound Notifications (Join / Leave Audio Cues)
  let soundCuesEnabled = localStorage.getItem('balabol_sound_cues') !== 'false';
  let previousStatusType = 'Disconnected';

  function playConnectSound(forced = false) {
    if (!soundCuesEnabled && !forced) return;
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      const now = ctx.currentTime;

      // Note 1: 587.33 Hz (D5) for 120ms
      const osc1 = ctx.createOscillator();
      const gain1 = ctx.createGain();
      osc1.type = 'sine';
      osc1.frequency.setValueAtTime(587.33, now);
      gain1.gain.setValueAtTime(0.001, now);
      gain1.gain.linearRampToValueAtTime(0.18, now + 0.02);
      gain1.gain.exponentialRampToValueAtTime(0.001, now + 0.14);
      osc1.connect(gain1);
      gain1.connect(ctx.destination);
      osc1.start(now);
      osc1.stop(now + 0.14);

      // Note 2: 880.0 Hz (A5) for 180ms
      const osc2 = ctx.createOscillator();
      const gain2 = ctx.createGain();
      osc2.type = 'sine';
      osc2.frequency.setValueAtTime(880.0, now + 0.08);
      gain2.gain.setValueAtTime(0.001, now + 0.08);
      gain2.gain.linearRampToValueAtTime(0.18, now + 0.10);
      gain2.gain.exponentialRampToValueAtTime(0.001, now + 0.28);
      osc2.connect(gain2);
      gain2.connect(ctx.destination);
      osc2.start(now + 0.08);
      osc2.stop(now + 0.28);
    } catch (e) {
      console.warn('Play connect sound error:', e);
    }
  }

  function playDisconnectSound(forced = false) {
    if (!soundCuesEnabled && !forced) return;
    try {
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      const now = ctx.currentTime;

      // Note 1: 659.25 Hz (E5) for 120ms
      const osc1 = ctx.createOscillator();
      const gain1 = ctx.createGain();
      osc1.type = 'sine';
      osc1.frequency.setValueAtTime(659.25, now);
      gain1.gain.setValueAtTime(0.001, now);
      gain1.gain.linearRampToValueAtTime(0.18, now + 0.02);
      gain1.gain.exponentialRampToValueAtTime(0.001, now + 0.14);
      osc1.connect(gain1);
      gain1.connect(ctx.destination);
      osc1.start(now);
      osc1.stop(now + 0.14);

      // Note 2: 440.0 Hz (A4) for 200ms
      const osc2 = ctx.createOscillator();
      const gain2 = ctx.createGain();
      osc2.type = 'sine';
      osc2.frequency.setValueAtTime(440.0, now + 0.08);
      gain2.gain.setValueAtTime(0.001, now + 0.08);
      gain2.gain.linearRampToValueAtTime(0.18, now + 0.10);
      gain2.gain.exponentialRampToValueAtTime(0.001, now + 0.32);
      osc2.connect(gain2);
      gain2.connect(ctx.destination);
      osc2.start(now + 0.08);
      osc2.stop(now + 0.32);
    } catch (e) {
      console.warn('Play disconnect sound error:', e);
    }
  }

  onMount(async () => {
    try {
      await invoke('set_mic_sensitivity', { sensitivity: micSensitivity / 100 });
      await invoke('set_mic_gain', { gain: micGain / 100 });
      await invoke('set_peer_volume', { volume: peerVolume / 100 });
    } catch (e) {
      console.error('Audio parameters init error:', e);
    }

    await pollState();
    // High-responsiveness state poll (150ms) for instant chat, status & speaking indicators
    pollInterval = setInterval(pollState, 150);
    // Independent video frame poll (120ms) - only transfers frames when changed
    videoPollInterval = setInterval(pollVideoFrame, 120);
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
    if (videoPollInterval) clearInterval(videoPollInterval);
    if (isMicTesting) {
      invoke('stop_mic_test').catch(() => {});
    }
    stopMediaStream();
  });

  async function pollState() {
    try {
      status = await invoke('get_status');
      peers = await invoke('get_peers');

      const prevType = previousStatusType;
      previousStatusType = status.type;

      if (prevType !== 'Connected' && status.type === 'Connected') {
        playConnectSound();
      } else if (prevType === 'Connected' && (status.type === 'Disconnected' || status.type === 'Failed')) {
        playDisconnectSound();
      }
      
      if (status.type === 'Connected') {
        const newChat = await invoke('get_chat_messages');
        if (newChat.length !== chatMessages.length) {
          chatMessages = newChat;
          scrollToChatBottom();
        }
      }

      // Poll voice status whenever connected OR while settings modal / mic test is active
      if (status.type === 'Connected' || showAudioSettingsModal || isMicTesting) {
        voiceStatus = await invoke('get_voice_status');
      }
    } catch (e) {
      console.error('Poll error:', e);
    }
  }

  async function pollVideoFrame() {
    if (status.type === 'Connected' && (voiceStatus.remote_media.video_active || voiceStatus.remote_media.screen_active)) {
      try {
        const frame = await invoke('get_remote_video_frame', { lastFrameId: lastReceivedFrameId });
        if (frame) {
          lastReceivedFrameId = frame.frame_id;
          remoteVideoFrame = frame;
        }
      } catch (e) {
        // ignore
      }
    }
  }

  function scrollToChatBottom() {
    setTimeout(() => {
      if (chatContainer) {
        chatContainer.scrollTop = chatContainer.scrollHeight;
      }
    }, 40);
  }

  function saveNickname(name) {
    nickname = name;
    localStorage.setItem('balabol_nickname', name);
  }

  // --- Voice Controls ---
  async function toggleMic() {
    const newMuted = !voiceStatus.mic_muted;
    await invoke('set_mic_muted', { muted: newMuted });
    voiceStatus.mic_muted = newMuted;
  }

  async function toggleDeafen() {
    const newDeafened = !voiceStatus.deafened;
    await invoke('set_deafened', { deafened: newDeafened });
    voiceStatus.deafened = newDeafened;
  }

  async function handleVolumeChange(e) {
    peerVolume = parseInt(e.target.value);
    localStorage.setItem('balabol_peer_vol', peerVolume.toString());
    await invoke('set_peer_volume', { volume: peerVolume / 100 });
  }

  async function handleSensitivityChange(e) {
    micSensitivity = parseInt(e.target.value);
    localStorage.setItem('balabol_mic_sens', micSensitivity.toString());
    await invoke('set_mic_sensitivity', { sensitivity: micSensitivity / 100 });
  }

  async function handleGainChange(e) {
    micGain = parseInt(e.target.value);
    localStorage.setItem('balabol_mic_gain', micGain.toString());
    await invoke('set_mic_gain', { gain: micGain / 100 });
  }

  async function handleRestartAudio() {
    audioRestartFeedback = 'Перезапуск...';
    try {
      await invoke('restart_audio');
      audioRestartFeedback = 'Успешно обновлено!';
      setTimeout(() => { audioRestartFeedback = ''; }, 2500);
    } catch (e) {
      audioRestartFeedback = 'Ошибка: ' + e;
      setTimeout(() => { audioRestartFeedback = ''; }, 3500);
    }
  }

  async function toggleMicTest() {
    isMicTesting = !isMicTesting;
    try {
      if (isMicTesting) {
        await invoke('start_mic_test');
      } else {
        await invoke('stop_mic_test');
      }
    } catch (e) {
      console.error('Mic test error:', e);
    }
  }

  // --- Video & Screen Share Controls ---
  async function toggleCamera() {
    if (isCameraOn) {
      stopMediaStream();
      return;
    }
    stopMediaStream();
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { max: 15 } },
        audio: false
      });
      localStream = stream;
      isCameraOn = true;
      isScreenOn = false;
      if (localVideoEl) {
        localVideoEl.srcObject = stream;
        localVideoEl.play().catch(() => {});
      }
      startVideoCapture(false);
      await invoke('send_media_state', { videoActive: true, screenActive: false });
    } catch (e) {
      console.error('Camera error:', e);
      alert('Не удалось получить доступ к веб-камере: ' + e.message);
    }
  }

  async function toggleScreenShare() {
    if (isScreenOn) {
      stopMediaStream();
      return;
    }
    stopMediaStream();
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: { frameRate: { max: 15 } },
        audio: false
      });
      localStream = stream;
      isScreenOn = true;
      isCameraOn = false;
      if (localVideoEl) {
        localVideoEl.srcObject = stream;
        localVideoEl.play().catch(() => {});
      }
      stream.getVideoTracks()[0].onended = () => {
        stopMediaStream();
      };
      startVideoCapture(true);
      await invoke('send_media_state', { videoActive: false, screenActive: true });
    } catch (e) {
      console.error('Screen share error:', e);
      if (e.name !== 'NotAllowedError') {
        alert('Не удалось запустить демонстрацию экрана: ' + e.message);
      }
    }
  }

  function stopMediaStream() {
    if (videoCaptureTimer) {
      clearInterval(videoCaptureTimer);
      videoCaptureTimer = null;
    }
    if (localStream) {
      localStream.getTracks().forEach(t => t.stop());
      localStream = null;
    }
    if (localVideoEl) {
      localVideoEl.srcObject = null;
    }
    const wasActive = isCameraOn || isScreenOn;
    isCameraOn = false;
    isScreenOn = false;
    if (wasActive && status.type === 'Connected') {
      invoke('send_media_state', { videoActive: false, screenActive: false }).catch(() => {});
    }
  }

  function startVideoCapture(isScreen) {
    if (videoCaptureTimer) clearInterval(videoCaptureTimer);
    // 180ms for screen (~5.5 fps, low packet burst, crisp text)
    // 120ms for webcam (~8 fps, smooth motion)
    videoCaptureTimer = setInterval(() => {
      captureAndSendVideoFrame(isScreen);
    }, isScreen ? 180 : 120);
  }

  function captureAndSendVideoFrame(isScreen) {
    if (!localVideoEl || !canvasEl || localVideoEl.videoWidth === 0) return;
    try {
      // 640x360 for screen, 480x360 for webcam
      const targetW = isScreen ? 640 : 480;
      const targetH = 360;
      canvasEl.width = targetW;
      canvasEl.height = targetH;
      const ctx = canvasEl.getContext('2d');
      ctx.drawImage(localVideoEl, 0, 0, targetW, targetH);
      // Quality 0.42 yields ~12-16 KB JPEG, keeping total network bandwidth under 50-80 KB/s
      const dataUrl = canvasEl.toDataURL('image/jpeg', isScreen ? 0.42 : 0.40);
      const commaIdx = dataUrl.indexOf(',');
      if (commaIdx !== -1) {
        const base64 = dataUrl.substring(commaIdx + 1);
        invoke('send_video_frame', { jpegBase64: base64, isScreen }).catch(() => {});
      }
    } catch (e) {
      console.error('Capture frame error:', e);
    }
  }

  // --- Actions: Room Code ---
  async function handleCreateRoom() {
    isSubmitting = true;
    try {
      saveNickname(nickname);
      const randomCode = Math.floor(100000 + Math.random() * 900000).toString();
      await invoke('start_room', {
        roomCode: randomCode,
        nickname: nickname.trim() || 'Хост'
      });
      showConnectModal = false;
    } catch (err) {
      alert('Ошибка создания комнаты: ' + err);
    } finally {
      isSubmitting = false;
    }
  }

  async function handleJoinRoom() {
    if (!roomCodeInput.trim()) {
      alert('Пожалуйста, введите код комнаты (например, 742918)');
      return;
    }
    isSubmitting = true;
    try {
      saveNickname(nickname);
      await invoke('join_room', {
        roomCode: roomCodeInput.trim(),
        nickname: nickname.trim() || 'Гость'
      });
      showConnectModal = false;
    } catch (err) {
      alert('Ошибка входа в комнату: ' + err);
    } finally {
      isSubmitting = false;
    }
  }

  // --- Actions: Chat (Optimistic 0ms Local Update) ---
  async function handleSendMessage() {
    const text = chatInput.trim();
    if (!text) return;
    chatInput = '';

    // Instant optimistic UI update (0ms delay!)
    const optimisticMsg = {
      id: Date.now() + Math.random(),
      sender: nickname.trim() || 'Я',
      text,
      timestamp_ms: Date.now(),
      is_me: true
    };
    chatMessages = [...chatMessages, optimisticMsg];
    scrollToChatBottom();

    try {
      await invoke('send_chat_message', {
        text,
        nickname: nickname.trim() || 'Я'
      });
      const updated = await invoke('get_chat_messages');
      chatMessages = updated;
      scrollToChatBottom();
    } catch (err) {
      console.error('Send error:', err);
    }
  }

  function handleChatKeyDown(e) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSendMessage();
    }
  }

  async function handleDisconnect() {
    stopMediaStream();
    remoteVideoFrame = null;
    lastReceivedFrameId = 0;
    if (status.type === 'Connected') {
      playDisconnectSound();
    }
    try {
      await invoke('disconnect');
      chatMessages = [];
    } catch (e) {
      console.error('Disconnect error:', e);
    }
  }

  function copyText(text, label) {
    navigator.clipboard.writeText(text);
    copiedItem = label;
    setTimeout(() => {
      copiedItem = '';
    }, 2000);
  }
</script>

<!-- Hidden Offscreen Canvas for Video Frame Encoding -->
<canvas bind:this={canvasEl} class="hidden"></canvas>

<div class="flex flex-col h-screen bg-[#111215] text-[#e0e2e6] font-sans select-none overflow-hidden">
  <!-- Top TeamSpeak-style Header Bar -->
  <header class="h-14 bg-[#181a1f] border-b border-[#26282e] flex items-center justify-between px-5 shrink-0 shadow-md">
    <div class="flex items-center gap-3">
      <div class="w-9 h-9 rounded-lg bg-gradient-to-tr from-blue-600 to-indigo-500 flex items-center justify-center shadow-lg shadow-blue-500/20">
        <Radio class="w-5 h-5 text-white" />
      </div>
      <div>
        <div class="flex items-center gap-2">
          <span class="font-bold text-base tracking-wide text-white">Balabol P2P</span>
          <span class="text-[10px] uppercase font-semibold tracking-wider bg-blue-500/10 text-blue-400 border border-blue-500/20 px-1.5 py-0.5 rounded">Голос + Видео</span>
        </div>
        <div class="text-xs text-[#8a8e98] flex items-center gap-1.5">
          <span>Ваш никнейм:</span>
          <span class="text-white font-medium">{nickname}</span>
          {#if voiceStatus.local_speaking}
            <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse ml-1" title="Микрофон передает голос"></span>
          {/if}
        </div>
      </div>
    </div>

    <!-- Right Controls / Status -->
    <div class="flex items-center gap-3">
      <!-- Audio Settings button (accessible in any state) -->
      <button
        on:click={() => showAudioSettingsModal = true}
        class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-[#22242e] hover:bg-[#2e313f] text-white text-xs font-semibold border border-[#323644] transition shadow"
        title="Настройки звука и чувствительности микрофона"
      >
        <Sliders class="w-3.5 h-3.5 text-blue-400" />
        <span class="hidden sm:inline">Звук</span>
      </button>

      {#if status.type === 'Disconnected'}
        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-red-500/10 text-red-400 border border-red-500/20 text-xs font-medium">
          <div class="w-2 h-2 rounded-full bg-red-500"></div>
          Отключен
        </div>
        <button
          on:click={() => showConnectModal = true}
          class="flex items-center gap-1.5 px-3.5 py-1.5 bg-blue-600 hover:bg-blue-500 text-white rounded-md text-xs font-semibold transition shadow-md shadow-blue-600/20"
        >
          <Server class="w-3.5 h-3.5" />
          Подключение
        </button>
      {:else if status.type === 'Hosting'}
        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-purple-500/10 text-purple-300 border border-purple-500/20 text-xs font-medium">
          <div class="w-2 h-2 rounded-full bg-purple-400 animate-pulse"></div>
          Комната: <span class="font-mono font-bold text-white tracking-widest">{status.room_code || ''}</span>
        </div>
        <button
          on:click={handleDisconnect}
          class="flex items-center gap-1.5 px-3 py-1.5 bg-red-600/80 hover:bg-red-600 text-white rounded-md text-xs font-semibold transition shadow-md shadow-red-600/20"
        >
          <PhoneOff class="w-3.5 h-3.5" />
          Закрыть комнату
        </button>
      {:else if status.type === 'Connecting'}
        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-amber-500/10 text-amber-300 border border-amber-500/20 text-xs font-medium">
          <RefreshCw class="w-3 h-3 text-amber-400 animate-spin" />
          {status.mode || 'Подключение...'}
        </div>
        <button
          on:click={handleDisconnect}
          class="px-3 py-1.5 bg-[#2b2d35] hover:bg-[#343740] text-white rounded-md text-xs font-semibold transition border border-[#3b3e48]"
        >
          Отмена
        </button>
      {:else if status.type === 'Connected'}
        <!-- Active Call Tabs -->
        <div class="flex items-center bg-[#101116] p-1 rounded-lg border border-[#2a2d36] text-xs">
          <button
            on:click={() => activeTab = 'stage'}
            class="px-3 py-1 rounded-md font-medium transition {activeTab === 'stage' ? 'bg-blue-600 text-white shadow' : 'text-[#8a8e98] hover:text-white'}"
          >
            Сцена
          </button>
          <button
            on:click={() => activeTab = 'split'}
            class="px-3 py-1 rounded-md font-medium transition {activeTab === 'split' ? 'bg-blue-600 text-white shadow' : 'text-[#8a8e98] hover:text-white'}"
          >
            Сплит
          </button>
          <button
            on:click={() => activeTab = 'chat'}
            class="px-3 py-1 rounded-md font-medium transition {activeTab === 'chat' ? 'bg-blue-600 text-white shadow' : 'text-[#8a8e98] hover:text-white'}"
          >
            Чат
          </button>
        </div>

        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-emerald-500/10 text-emerald-400 border border-emerald-500/20 text-xs font-medium">
          <div class="w-2 h-2 rounded-full {voiceStatus.remote_speaking ? 'bg-emerald-400 animate-ping' : 'bg-emerald-500'}"></div>
          {status.peer_name}
          <span class="text-xs bg-emerald-950/60 px-1.5 py-0.2 rounded text-emerald-300 border border-emerald-500/30 font-mono">
            {status.ping_ms} мс
          </span>
        </div>
        <button
          on:click={handleDisconnect}
          class="flex items-center gap-1.5 px-3 py-1.5 bg-red-600/80 hover:bg-red-600 text-white rounded-md text-xs font-semibold transition shadow-md shadow-red-600/20"
        >
          <PhoneOff class="w-3.5 h-3.5" />
          Отключиться
        </button>
      {:else if status.type === 'Failed'}
        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-red-500/10 text-red-400 border border-red-500/20 text-xs font-medium">
          <AlertCircle class="w-3.5 h-3.5" />
          Ошибка связи
        </div>
        <button
          on:click={handleDisconnect}
          class="px-3 py-1.5 bg-[#2b2d35] hover:bg-[#343740] text-white rounded-md text-xs font-semibold transition border border-[#3b3e48]"
        >
          Сброс
        </button>
      {/if}
    </div>
  </header>

  <!-- Main Body Layout -->
  <div class="flex-1 flex overflow-hidden">
    <!-- Center Content -->
    <main class="flex-1 flex flex-col p-4 overflow-y-auto bg-[#14151a]">
      {#if status.type === 'Disconnected'}
        <!-- Disconnected Zero State -->
        <div class="max-w-xl mx-auto w-full my-auto flex flex-col items-center text-center">
          <div class="w-16 h-16 rounded-2xl bg-[#1c1e24] border border-[#2a2d36] flex items-center justify-center mb-5 text-blue-400 shadow-xl">
            <Radio class="w-8 h-8" />
          </div>
          <h2 class="text-2xl font-bold text-white mb-2">Голосовая и видео P2P связь</h2>
          <p class="text-sm text-[#8a8e98] mb-8 max-w-md leading-relaxed">
            Прямое соединение без серверов. Чистый нативный звук с VAD-сглаживанием, передача веб-камеры и рабочего стола.
          </p>

          <div class="grid grid-cols-1 md:grid-cols-2 gap-4 w-full">
            <!-- Card 1: Fast Create -->
            <button
              on:click={handleCreateRoom}
              disabled={isSubmitting}
              class="flex flex-col items-start p-5 rounded-xl bg-[#1a1c22] border border-[#262832] hover:border-blue-500/40 hover:bg-[#1e2028] transition group text-left shadow-lg disabled:opacity-50"
            >
              <div class="w-10 h-10 rounded-lg bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400 mb-3 group-hover:scale-105 transition">
                <Key class="w-5 h-5" />
              </div>
              <h3 class="text-base font-semibold text-white mb-1">Создать комнату</h3>
              <p class="text-xs text-[#8a8e98] mb-4">
                Сгенерирует 6-значный код комнаты для звонка с собеседником.
              </p>
              <div class="mt-auto flex items-center gap-1.5 text-xs font-semibold text-blue-400">
                Получить код <ArrowRight class="w-3.5 h-3.5" />
              </div>
            </button>

            <!-- Card 2: Join by Code -->
            <div class="flex flex-col p-5 rounded-xl bg-[#1a1c22] border border-[#262832] text-left shadow-lg">
              <div class="w-10 h-10 rounded-lg bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400 mb-3">
                <Link class="w-5 h-5" />
              </div>
              <h3 class="text-base font-semibold text-white mb-1">Войти по коду</h3>
              <p class="text-xs text-[#8a8e98] mb-3">
                Введите код, который передал создатель комнаты:
              </p>
              <div class="mt-auto space-y-2">
                <input
                  type="text"
                  bind:value={roomCodeInput}
                  placeholder="6 цифр кода"
                  maxlength="8"
                  class="w-full bg-[#101116] border border-[#282b36] rounded-lg px-3 py-1.5 text-white font-mono text-center tracking-widest text-sm focus:outline-none focus:border-emerald-500"
                />
                <button
                  on:click={handleJoinRoom}
                  disabled={isSubmitting || !roomCodeInput.trim()}
                  class="w-full py-1.5 bg-emerald-600 hover:bg-emerald-500 disabled:opacity-40 text-white rounded-lg text-xs font-semibold transition shadow-md shadow-emerald-600/20"
                >
                  Войти в звонок
                </button>
              </div>
            </div>
          </div>
        </div>

      {:else if status.type === 'Hosting'}
        <!-- Hosting State -->
        <div class="max-w-2xl mx-auto w-full space-y-6 my-auto">
          <div class="p-6 rounded-2xl bg-gradient-to-r from-purple-950/40 to-[#1b1c24] border border-purple-500/20 shadow-xl text-center space-y-5">
            <div class="inline-flex items-center gap-2 text-xs font-bold uppercase tracking-wider text-purple-400 px-3 py-1 rounded-full bg-purple-500/10 border border-purple-500/20">
              <span class="w-2 h-2 rounded-full bg-purple-400 animate-ping"></span>
              Ожидание подключения собеседника к звонку
            </div>

            <div>
              <span class="text-xs text-[#8a8e98] block mb-2">Передайте этот 6-значный код собеседнику:</span>
              <div class="inline-flex items-center gap-3 bg-[#101116] border border-purple-500/40 rounded-2xl px-6 py-3 shadow-inner">
                <span class="text-4xl font-mono font-extrabold text-white tracking-[0.25em]">
                  {status.room_code || '------'}
                </span>
                <button
                  on:click={() => copyText(status.room_code, 'code')}
                  class="p-2.5 bg-purple-600 hover:bg-purple-500 text-white rounded-xl transition shadow-md"
                  title="Скопировать код"
                >
                  {#if copiedItem === 'code'}
                    <Check class="w-5 h-5 text-emerald-300" />
                  {:else}
                    <Copy class="w-5 h-5" />
                  {/if}
                </button>
              </div>
              {#if copiedItem === 'code'}
                <div class="text-xs text-emerald-400 font-semibold mt-2">Код скопирован в буфер!</div>
              {/if}
            </div>

            <!-- Diagnostics -->
            <div class="pt-3 border-t border-[#262832] grid grid-cols-2 gap-3 text-xs text-left">
              <div class="flex items-center gap-2 text-[#9ba0ad] bg-[#14151b] p-2.5 rounded-lg border border-[#22242e]">
                <Wifi class="w-4 h-4 text-blue-400 shrink-0" />
                <div class="truncate">
                  <span class="text-[#646875] block text-[10px]">LAN IP:</span>
                  <span class="font-mono text-white text-[11px]">{status.local_addr || 'Нет'}</span>
                </div>
              </div>

              <div class="flex items-center gap-2 text-[#9ba0ad] bg-[#14151b] p-2.5 rounded-lg border border-[#22242e]">
                <Globe class="w-4 h-4 text-emerald-400 shrink-0" />
                <div class="truncate">
                  <span class="text-[#646875] block text-[10px]">STUN / Публичный IP:</span>
                  <span class="font-mono text-white text-[11px]">{status.public_addr || 'Определяется...'}</span>
                </div>
              </div>
            </div>
          </div>
        </div>

      {:else if status.type === 'Connecting'}
        <!-- Connecting State -->
        <div class="max-w-md mx-auto w-full my-auto text-center p-8 rounded-2xl bg-[#181a20] border border-[#282b36] shadow-2xl">
          <div class="w-16 h-16 rounded-full bg-amber-500/10 border border-amber-500/20 flex items-center justify-center mx-auto mb-5 text-amber-400">
            <RefreshCw class="w-8 h-8 animate-spin" />
          </div>
          <h2 class="text-xl font-bold text-white mb-2">P2P Согласование и пробитие NAT</h2>
          <p class="text-xs text-amber-300 font-semibold mb-4 bg-amber-500/10 py-1.5 px-3 rounded-lg inline-block border border-amber-500/20">
            {status.mode || 'Встречный пробив портов...'}
          </p>
          <p class="text-xs text-[#8a8e98] mb-6">
            Цель: <span class="text-white font-semibold">{status.target_str}</span>
          </p>
          <button
            on:click={handleDisconnect}
            class="w-full py-2.5 bg-[#262832] hover:bg-[#313440] text-white rounded-lg text-xs font-semibold transition"
          >
            Прервать подключение
          </button>
        </div>

      {:else if status.type === 'Connected'}
        <!-- Connected Discord-Style Video & Audio Call Space -->
        <div class="w-full h-full flex flex-col gap-3 overflow-hidden">
          
          <!-- Content Area: Stage, Split, or Chat -->
          <div class="flex-1 flex gap-3 min-h-0 overflow-hidden">
            
            <!-- STAGE / VIDEO CALL AREA -->
            {#if activeTab === 'stage' || activeTab === 'split'}
              <div class="flex-1 relative flex flex-col bg-[#16181f] border border-[#262832] rounded-2xl overflow-hidden shadow-2xl min-w-0">
                
                <!-- Main Stage Canvas / Video View -->
                <div class="flex-1 relative bg-[#0b0c0f] flex items-center justify-center overflow-hidden">
                  {#if remoteVideoFrame && (voiceStatus.remote_media.video_active || voiceStatus.remote_media.screen_active)}
                    <!-- Remote Video Stream -->
                    <div class="relative w-full h-full flex items-center justify-center bg-black">
                      <img
                        src="data:image/jpeg;base64,{remoteVideoFrame.jpeg_base64}"
                        alt="Remote Feed"
                        class="w-full h-full object-contain"
                      />
                      <div class="absolute top-4 left-4 bg-black/70 backdrop-blur-md px-3 py-1.5 rounded-lg border border-white/10 flex items-center gap-2 text-xs font-semibold text-white">
                        {#if remoteVideoFrame.is_screen}
                          <Monitor class="w-4 h-4 text-purple-400" />
                          <span>Экран: {status.peer_name}</span>
                        {:else}
                          <Video class="w-4 h-4 text-emerald-400" />
                          <span>Камера: {status.peer_name}</span>
                        {/if}
                      </div>

                      <div class="absolute top-4 right-4 flex items-center gap-2">
                        <button
                          on:click={() => activeTab = activeTab === 'stage' ? 'split' : 'stage'}
                          class="bg-black/70 hover:bg-black/90 backdrop-blur-md px-3 py-1.5 rounded-lg border border-white/15 flex items-center gap-1.5 text-xs font-semibold text-white transition shadow"
                          title={activeTab === 'stage' ? 'Вернуться в сплит' : 'Развернуть во весь экран'}
                        >
                          <Maximize2 class="w-3.5 h-3.5 text-blue-400" />
                          <span>{activeTab === 'stage' ? 'Сплит' : 'Во весь экран'}</span>
                        </button>
                      </div>
                    </div>
                  {:else}
                    <!-- Remote Audio Tile (Discord style) -->
                    <div class="flex flex-col items-center justify-center text-center p-8">
                      <div class="relative mb-4">
                        <div class="w-28 h-28 rounded-full bg-gradient-to-tr from-indigo-600 to-purple-600 flex items-center justify-center text-3xl font-extrabold text-white shadow-2xl transition duration-200 {voiceStatus.remote_speaking ? 'ring-4 ring-emerald-500 scale-105 shadow-emerald-500/40' : 'ring-2 ring-white/10'}">
                          {status.peer_name.slice(0, 2).toUpperCase()}
                        </div>
                        {#if voiceStatus.remote_speaking}
                          <div class="absolute -top-1 -right-1 w-6 h-6 rounded-full bg-emerald-500 border-2 border-[#0b0c0f] flex items-center justify-center animate-bounce shadow">
                            <Volume2 class="w-3.5 h-3.5 text-white" />
                          </div>
                        {/if}
                        {#if voiceStatus.remote_media.mic_muted}
                          <div class="absolute -bottom-1 -right-1 w-6 h-6 rounded-full bg-red-600 border-2 border-[#0b0c0f] flex items-center justify-center shadow">
                            <MicOff class="w-3 h-3 text-white" />
                          </div>
                        {/if}
                      </div>

                      <div class="text-lg font-bold text-white flex items-center gap-2">
                        <span>{status.peer_name}</span>
                        {#if voiceStatus.remote_speaking}
                          <span class="text-xs bg-emerald-500/20 text-emerald-400 px-2 py-0.5 rounded-full font-semibold">Говорит</span>
                        {/if}
                      </div>
                      <div class="text-xs text-[#707482] mt-1 font-mono">{status.peer_addr}</div>
                    </div>
                  {/if}

                  <!-- Local Picture-in-Picture (PiP) Preview -->
                  <div class="absolute bottom-4 right-4 z-20 transition-all {isCameraOn || isScreenOn ? 'block' : 'hidden'}">
                    <div class="relative rounded-xl overflow-hidden border border-white/20 shadow-2xl bg-[#111215]">
                      <video
                        bind:this={localVideoEl}
                        autoplay
                        muted
                        playsinline
                        class="w-48 h-32 object-cover"
                      ></video>
                      <div class="absolute top-2 left-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] text-white font-medium flex items-center gap-1">
                        {#if isScreenOn}
                          <Monitor class="w-3 h-3 text-purple-400" /> Экран
                        {:else}
                          <Video class="w-3 h-3 text-blue-400" /> Вы
                        {/if}
                      </div>
                    </div>
                  </div>
                </div>

                <!-- Floating Bottom Control Dock (Discord Style) -->
                <div class="p-3 bg-[#13141a]/95 backdrop-blur-md border-t border-[#262832] flex items-center justify-between shrink-0">
                  <!-- Left side: speaking status -->
                  <div class="flex items-center gap-3">
                    <div class="flex items-center gap-2">
                      <div class="w-3 h-3 rounded-full {voiceStatus.local_speaking ? 'bg-emerald-400 ring-2 ring-emerald-500/50 animate-pulse' : 'bg-[#3b3e48]'}"></div>
                      <span class="text-xs text-[#8a8e98] font-medium">
                        {voiceStatus.mic_muted ? 'Микрофон отключен' : voiceStatus.local_speaking ? 'Передача голоса...' : 'Микрофон активен'}
                      </span>
                    </div>
                  </div>

                  <!-- Center: Action Buttons -->
                  <div class="flex items-center gap-2">
                    <!-- Mic Toggle -->
                    <button
                      on:click={toggleMic}
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {voiceStatus.mic_muted ? 'bg-red-600 hover:bg-red-500 text-white' : voiceStatus.local_speaking ? 'bg-emerald-600 hover:bg-emerald-500 text-white ring-2 ring-emerald-400' : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={voiceStatus.mic_muted ? 'Включить микрофон' : 'Отключить микрофон'}
                    >
                      {#if voiceStatus.mic_muted}
                        <MicOff class="w-5 h-5" />
                      {:else}
                        <Mic class="w-5 h-5" />
                      {/if}
                    </button>

                    <!-- Deafen Toggle -->
                    <button
                      on:click={toggleDeafen}
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {voiceStatus.deafened ? 'bg-red-600 hover:bg-red-500 text-white' : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={voiceStatus.deafened ? 'Включить звук' : 'Заглушить звук (дефен)'}
                    >
                      {#if voiceStatus.deafened}
                        <VolumeX class="w-5 h-5" />
                      {:else}
                        <Headphones class="w-5 h-5" />
                      {/if}
                    </button>

                    <!-- Camera Toggle -->
                    <button
                      on:click={toggleCamera}
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {isCameraOn ? 'bg-blue-600 hover:bg-blue-500 text-white ring-2 ring-blue-400' : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={isCameraOn ? 'Выключить веб-камеру' : 'Включить веб-камеру'}
                    >
                      {#if isCameraOn}
                        <Video class="w-5 h-5" />
                      {:else}
                        <VideoOff class="w-5 h-5" />
                      {/if}
                    </button>

                    <!-- Screen Share Toggle -->
                    <button
                      on:click={toggleScreenShare}
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {isScreenOn ? 'bg-purple-600 hover:bg-purple-500 text-white ring-2 ring-purple-400' : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={isScreenOn ? 'Остановить демонстрацию' : 'Поделиться экраном'}
                    >
                      {#if isScreenOn}
                        <Monitor class="w-5 h-5" />
                      {:else}
                        <MonitorOff class="w-5 h-5" />
                      {/if}
                    </button>

                    <!-- Disconnect -->
                    <button
                      on:click={handleDisconnect}
                      class="p-3 bg-red-600 hover:bg-red-500 text-white rounded-full transition shadow-lg shadow-red-600/30 flex items-center justify-center ml-2"
                      title="Завершить звонок"
                    >
                      <PhoneOff class="w-5 h-5" />
                    </button>
                  </div>

                  <!-- Right: Volume Slider / Settings -->
                  <div class="relative flex items-center gap-2">
                    <button
                      on:click={() => showAudioSettingsModal = true}
                      class="p-2.5 rounded-lg bg-[#22242e] hover:bg-[#2c2f3c] text-white text-xs flex items-center gap-1.5 transition"
                      title="Настройки чувствительности микрофона и звука"
                    >
                      <Sliders class="w-4 h-4 text-purple-400" />
                      <span class="hidden sm:inline">Звук</span>
                    </button>

                    <button
                      on:click={() => showVolumeSlider = !showVolumeSlider}
                      class="p-2.5 rounded-lg bg-[#22242e] hover:bg-[#2c2f3c] text-white text-xs flex items-center gap-1.5 transition"
                      title="Громкость собеседника"
                    >
                      <Volume2 class="w-4 h-4 text-blue-400" />
                      <span class="font-mono">{peerVolume}%</span>
                    </button>

                    {#if showVolumeSlider}
                      <div class="absolute bottom-12 right-0 bg-[#1e2028] border border-[#2d303e] rounded-xl p-3 shadow-2xl w-48 z-50 space-y-2">
                        <div class="text-[11px] font-semibold text-[#8a8e98] flex justify-between">
                          <span>Громкость собеседника</span>
                          <span class="text-white font-mono">{peerVolume}%</span>
                        </div>
                        <input
                          type="range"
                          min="0"
                          max="200"
                          bind:value={peerVolume}
                          on:input={handleVolumeChange}
                          class="w-full accent-blue-500"
                        />
                      </div>
                    {/if}
                  </div>

                </div>
              </div>
            {/if}

            <!-- P2P TEXT CHAT -->
            {#if activeTab === 'chat' || activeTab === 'split'}
              <div class="{activeTab === 'split' ? 'w-80 md:w-96' : 'flex-1'} flex flex-col bg-[#16181f] border border-[#262832] rounded-2xl overflow-hidden shadow-2xl shrink-0">
                <!-- Chat Header -->
                <div class="h-11 bg-[#1a1c24] border-b border-[#262832] px-4 flex items-center justify-between text-xs font-semibold text-[#8a8e98]">
                  <div class="flex items-center gap-2 text-white">
                    <MessageSquare class="w-4 h-4 text-blue-400" />
                    <span>P2P Текстовый чат</span>
                  </div>
                  <span class="text-[11px] text-[#606470]">{chatMessages.length} сообщ.</span>
                </div>

                <!-- Messages Stream -->
                <div
                  bind:this={chatContainer}
                  class="flex-1 p-3 overflow-y-auto space-y-2.5 font-sans select-text"
                >
                  {#if chatMessages.length === 0}
                    <div class="h-full flex flex-col items-center justify-center text-center text-xs text-[#606470] space-y-2">
                      <MessageSquare class="w-8 h-8 text-[#303340]" />
                      <span>Канал связи открыт! Напишите сообщение.</span>
                    </div>
                  {:else}
                    {#each chatMessages as msg (msg.id + '-' + (msg.is_me ? 'me' : 'peer') + '-' + msg.timestamp_ms)}
                      <div class="flex flex-col {msg.is_me ? 'items-end' : 'items-start'}">
                        <div class="flex items-center gap-1.5 mb-1 px-1 text-[10px] text-[#707482]">
                          <span class="font-semibold text-white">{msg.sender}</span>
                          <span>•</span>
                          <span>{new Date(msg.timestamp_ms).toLocaleTimeString()}</span>
                        </div>
                        <div
                          class="max-w-[85%] px-3 py-1.5 rounded-2xl text-xs leading-relaxed shadow-md break-words {msg.is_me ? 'bg-blue-600 text-white rounded-tr-none' : 'bg-[#222530] text-[#e0e2e8] border border-[#2d303e] rounded-tl-none'}"
                        >
                          {msg.text}
                        </div>
                      </div>
                    {/each}
                  {/if}
                </div>

                <!-- Chat Input Area -->
                <div class="p-2.5 bg-[#13141a] border-t border-[#262832] flex items-center gap-2">
                  <input
                    type="text"
                    bind:value={chatInput}
                    on:keydown={handleChatKeyDown}
                    placeholder="Сообщение..."
                    class="flex-1 bg-[#1a1c24] border border-[#2b2d38] rounded-xl px-3 py-2 text-xs text-white placeholder-[#555866] focus:outline-none focus:border-blue-500 transition"
                  />
                  <button
                    on:click={handleSendMessage}
                    disabled={!chatInput.trim()}
                    class="p-2 bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-white rounded-xl transition shadow-md shadow-blue-600/20"
                    title="Отправить"
                  >
                    <Send class="w-4 h-4" />
                  </button>
                </div>
              </div>
            {/if}

          </div>

        </div>

      {:else if status.type === 'Failed'}
        <!-- Failed State -->
        <div class="max-w-md mx-auto w-full my-auto text-center p-8 rounded-2xl bg-[#181a20] border border-red-500/30 shadow-2xl">
          <div class="w-16 h-16 rounded-full bg-red-500/10 border border-red-500/20 flex items-center justify-center mx-auto mb-5 text-red-400">
            <AlertCircle class="w-8 h-8" />
          </div>
          <h2 class="text-xl font-bold text-white mb-2">Не удалось связаться</h2>
          <p class="text-xs text-red-300/80 mb-6 bg-red-950/30 p-3 rounded-lg border border-red-500/20 leading-relaxed text-left">
            {status.error}
          </p>
          <div class="space-y-2">
            <button
              on:click={() => { handleDisconnect(); showConnectModal = true; }}
              class="w-full py-2.5 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-xs font-semibold transition shadow-md"
            >
              Попробовать снова
            </button>
            <button
              on:click={handleDisconnect}
              class="w-full py-2 bg-[#262832] hover:bg-[#313440] text-white rounded-lg text-xs font-semibold transition"
            >
              Закрыть
            </button>
          </div>
        </div>
      {/if}
    </main>
  </div>

  <!-- Unified Room Code Connection Modal -->
  {#if showConnectModal}
    <div class="fixed inset-0 bg-black/75 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div class="bg-[#181a20] border border-[#2a2d38] w-full max-w-md rounded-2xl p-6 shadow-2xl space-y-5">
        <div class="flex items-center justify-between border-b border-[#262832] pb-4">
          <h3 class="text-lg font-bold text-white flex items-center gap-2">
            <Key class="w-5 h-5 text-blue-400" />
            Подключение по коду
          </h3>
          <button on:click={() => showConnectModal = false} class="text-[#8a8e98] hover:text-white text-lg">✕</button>
        </div>

        <div>
          <label for="modal-nick" class="block text-xs font-semibold text-[#8a8e98] mb-1">Ваш никнейм</label>
          <input
            id="modal-nick"
            bind:value={nickname}
            class="w-full bg-[#101116] border border-[#282b36] rounded-lg px-3 py-2 text-xs text-white focus:outline-none focus:border-blue-500"
            placeholder="Никнейм"
          />
        </div>

        <div class="space-y-4 text-xs pt-1">
          <!-- Option A: Create -->
          <div class="p-4 rounded-xl bg-[#14151b] border border-[#242732] space-y-2.5">
            <h4 class="font-bold text-white text-sm">Создать новую комнату</h4>
            <p class="text-[#8a8e98]">
              Нажмите, чтобы сгенерировать 6-значный код комнаты и ожидать гостя.
            </p>
            <button
              disabled={isSubmitting}
              on:click={handleCreateRoom}
              class="w-full py-2.5 bg-blue-600 hover:bg-blue-500 text-white rounded-lg font-semibold transition shadow-md shadow-blue-600/20 disabled:opacity-50"
            >
              {isSubmitting ? 'Создание...' : 'Создать комнату и получить код'}
            </button>
          </div>

          <!-- Option B: Join -->
          <div class="p-4 rounded-xl bg-[#14151b] border border-[#242732] space-y-2.5">
            <h4 class="font-bold text-white text-sm">Войти по коду</h4>
            <div>
              <label for="room-code-input-modal" class="block text-[#8a8e98] mb-1 font-semibold">6-значный код комнаты</label>
              <input
                id="room-code-input-modal"
                bind:value={roomCodeInput}
                placeholder="Например, 742918"
                class="w-full bg-[#101116] border border-[#282b36] rounded-lg px-3 py-2 text-white font-mono text-center text-base tracking-widest focus:outline-none focus:border-blue-500"
              />
            </div>
            <button
              disabled={isSubmitting || !roomCodeInput.trim()}
              on:click={handleJoinRoom}
              class="w-full py-2.5 bg-emerald-600 hover:bg-emerald-500 disabled:opacity-40 text-white rounded-lg font-semibold transition shadow-md shadow-emerald-600/20"
            >
              {isSubmitting ? 'Подключение...' : 'Войти в комнату'}
            </button>
          </div>
        </div>

        <div class="flex justify-end pt-2">
          <button
            on:click={() => showConnectModal = false}
            class="px-4 py-2 bg-[#22242e] hover:bg-[#2b2d38] text-white rounded-lg text-xs font-semibold transition"
          >
            Закрыть
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Audio & Microphone Settings Modal -->
  {#if showAudioSettingsModal}
    <div class="fixed inset-0 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div class="bg-[#181a20] border border-[#2b2e3b] w-full max-w-lg rounded-2xl shadow-2xl overflow-hidden flex flex-col max-h-[90vh]">
        
        <!-- Modal Header -->
        <div class="px-6 py-4 bg-[#1b1d24] border-b border-[#272a36] flex items-center justify-between shrink-0">
          <div class="flex items-center gap-2.5 text-white font-bold text-base">
            <div class="w-8 h-8 rounded-lg bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400">
              <Sliders class="w-4 h-4" />
            </div>
            <span>Настройки звука и микрофона</span>
          </div>
          <button
            on:click={() => { showAudioSettingsModal = false; if (isMicTesting) toggleMicTest(); }}
            class="text-[#8a8e98] hover:text-white p-1 rounded-lg hover:bg-white/10 transition"
          >
            <X class="w-5 h-5" />
          </button>
        </div>

        <!-- Modal Body -->
        <div class="p-6 space-y-5 overflow-y-auto">

          <!-- 1. Microphone Sensitivity & Live Meter -->
          <div class="space-y-3 bg-[#13141a] p-4 rounded-xl border border-[#252834]">
            <div class="flex items-center justify-between">
              <label for="mic-sens-range" class="text-xs font-semibold text-white flex items-center gap-2">
                <Mic class="w-4 h-4 text-emerald-400" />
                Чувствительность микрофона (VAD Порог)
              </label>
              <span class="text-xs font-mono font-bold text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded border border-emerald-500/20">
                {micSensitivity}%
              </span>
            </div>

            <p class="text-[11px] text-[#8a8e98] leading-relaxed">
              Порог активации голоса. Настройте так, чтобы фоновый шум оставался слева от белой отметки, а голос пересекал её в зеленую зону.
            </p>

            <!-- Live Audio Meter Bar with Threshold Indicator -->
            <div class="space-y-1.5 pt-1">
              <div class="relative w-full h-4 bg-[#0d0e12] rounded-full overflow-hidden border border-[#262832]">
                <!-- Dynamic Level Bar -->
                <div
                  class="h-full rounded-full transition-all duration-75 {((voiceStatus.mic_level || 0) * 100) >= micSensitivity ? 'bg-gradient-to-r from-emerald-500 to-green-400 shadow-lg shadow-emerald-500/50' : 'bg-gradient-to-r from-[#2a2d38] to-[#3a3f50]'}"
                  style="width: {Math.min(100, Math.max(0, (voiceStatus.mic_level || 0) * 100))}%"
                ></div>

                <!-- Threshold Marker Pin -->
                <div
                  class="absolute top-0 bottom-0 w-1 bg-white shadow-md z-10"
                  style="left: {micSensitivity}%;"
                  title="Порог срабатывания: {micSensitivity}%"
                ></div>
              </div>

              <!-- Meter Legend & Speaking Badge -->
              <div class="flex items-center justify-between text-[11px] text-[#6b7080]">
                <span>Тишина / Шум</span>
                <div class="flex items-center gap-1.5 font-medium {((voiceStatus.mic_level || 0) * 100) >= micSensitivity ? 'text-emerald-400 font-bold' : 'text-[#6b7080]'}">
                  <span class="w-2 h-2 rounded-full {((voiceStatus.mic_level || 0) * 100) >= micSensitivity ? 'bg-emerald-400 animate-pulse' : 'bg-[#3b3e48]'}"></span>
                  <span>{((voiceStatus.mic_level || 0) * 100) >= micSensitivity ? 'Голос активен (передача)' : 'Ожидание голоса'}</span>
                </div>
                <span>Громко</span>
              </div>
            </div>

            <!-- Sensitivity Range Slider -->
            <div class="pt-1">
              <input
                id="mic-sens-range"
                type="range"
                min="0"
                max="100"
                bind:value={micSensitivity}
                on:input={handleSensitivityChange}
                class="w-full accent-emerald-500 cursor-pointer"
              />
              <div class="flex justify-between text-[10px] text-[#6b7080] mt-1 font-medium">
                <span>0% (Высокая / шепот)</span>
                <span>50% (Стандарт)</span>
                <span>100% (Шумоподавление)</span>
              </div>
            </div>
          </div>

          <!-- 2. Mic Gain & Echo Test -->
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <!-- Mic Gain -->
            <div class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-2">
              <div class="flex justify-between items-center text-xs font-semibold text-white">
                <label for="mic-gain-range">Усиление микрофона</label>
                <span class="font-mono text-blue-400">{micGain}%</span>
              </div>
              <input
                id="mic-gain-range"
                type="range"
                min="50"
                max="200"
                step="5"
                bind:value={micGain}
                on:input={handleGainChange}
                class="w-full accent-blue-500 cursor-pointer"
              />
              <p class="text-[10px] text-[#787c8a]">
                Если микрофон звучит тихо, повысьте усиление до 130-160%.
              </p>
            </div>

            <!-- Mic Test Toggle -->
            <div class="bg-[#13141a] p-4 rounded-xl border border-[#252834] flex flex-col justify-between">
              <div>
                <div class="text-xs font-semibold text-white mb-1">Проверка микрофона</div>
                <p class="text-[10px] text-[#787c8a]">
                  Слушайте себя в наушниках для проверки чистоты звука и громкости.
                </p>
              </div>
              <button
                on:click={toggleMicTest}
                class="mt-3 w-full py-2 px-3 rounded-lg text-xs font-semibold transition flex items-center justify-center gap-2 {isMicTesting ? 'bg-amber-600 hover:bg-amber-500 text-white ring-2 ring-amber-400' : 'bg-[#22242e] hover:bg-[#2c2f3c] text-white border border-[#343847]'}"
              >
                <Headphones class="w-3.5 h-3.5" />
                <span>{isMicTesting ? 'Остановить проверку' : 'Слышать себя (Тест)'}</span>
              </button>
            </div>
          </div>

          <!-- 3. Output Volume -->
          <div class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-2">
            <div class="flex justify-between items-center text-xs font-semibold text-white">
              <label for="peer-vol-range" class="flex items-center gap-1.5">
                <Volume2 class="w-4 h-4 text-blue-400" />
                Громкость собеседника
              </label>
              <span class="font-mono text-blue-400">{peerVolume}%</span>
            </div>
            <input
              id="peer-vol-range"
              type="range"
              min="0"
              max="200"
              bind:value={peerVolume}
              on:input={handleVolumeChange}
              class="w-full accent-blue-500 cursor-pointer"
            />
          </div>

          <!-- Sound Notifications (Connect / Disconnect cues) -->
          <div class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-3">
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-2">
                <Bell class="w-4 h-4 text-amber-400" />
                <span class="text-xs font-semibold text-white">Звуковые сигналы входа и выхода</span>
              </div>
              <label class="relative inline-flex items-center cursor-pointer">
                <input
                  type="checkbox"
                  checked={soundCuesEnabled}
                  on:change={(e) => {
                    soundCuesEnabled = e.target.checked;
                    localStorage.setItem('balabol_sound_cues', soundCuesEnabled ? 'true' : 'false');
                  }}
                  class="sr-only peer"
                />
                <div class="w-9 h-5 bg-[#252834] peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
              </label>
            </div>
            <p class="text-[10px] text-[#787c8a]">
              Воспроизводить звуковые сигналы (TeamSpeak / Discord стиль), когда собеседник подключается или покидает голосовой чат.
            </p>
            <div class="flex gap-2 pt-1">
              <button
                on:click={() => playConnectSound(true)}
                class="py-1.5 px-3 bg-[#20222c] hover:bg-[#2b2e3c] text-white rounded-lg text-xs font-medium transition flex items-center gap-1.5 border border-[#303342]"
              >
                <span>🔔 Вход (тест)</span>
              </button>
              <button
                on:click={() => playDisconnectSound(true)}
                class="py-1.5 px-3 bg-[#20222c] hover:bg-[#2b2e3c] text-white rounded-lg text-xs font-medium transition flex items-center gap-1.5 border border-[#303342]"
              >
                <span>🔕 Выход (тест)</span>
              </button>
            </div>
          </div>

          <!-- 4. Device Status & Reconnect / Recovery -->
          <div class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-3">
            <div class="text-xs font-semibold text-white flex items-center justify-between">
              <span>Аудио устройства и авто-переключение</span>
              <span class="text-[10px] text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded border border-emerald-500/20">
                Авто-восстановление активно
              </span>
            </div>

            <div class="grid grid-cols-1 gap-2 text-xs">
              <div class="flex items-center justify-between p-2.5 rounded-lg bg-[#0d0e12] border border-[#1f212a]">
                <div class="flex items-center gap-2 truncate pr-2">
                  <Headphones class="w-4 h-4 text-purple-400 shrink-0" />
                  <span class="text-[#8a8e98] text-[11px] shrink-0">Вывод (Наушники):</span>
                  <span class="text-white text-[11px] font-mono truncate">{voiceStatus.output_device_name || 'По умолчанию (Windows)'}</span>
                </div>
                <span class="text-[10px] px-2 py-0.5 rounded font-medium shrink-0 {voiceStatus.output_device_ok ? 'text-emerald-400 bg-emerald-500/10' : 'text-amber-400 bg-amber-500/10'}">
                  {voiceStatus.output_device_ok ? 'Подключено' : 'Переподключение...'}
                </span>
              </div>

              <div class="flex items-center justify-between p-2.5 rounded-lg bg-[#0d0e12] border border-[#1f212a]">
                <div class="flex items-center gap-2 truncate pr-2">
                  <Mic class="w-4 h-4 text-emerald-400 shrink-0" />
                  <span class="text-[#8a8e98] text-[11px] shrink-0">Ввод (Микрофон):</span>
                  <span class="text-white text-[11px] font-mono truncate">{voiceStatus.input_device_name || 'По умолчанию (Windows)'}</span>
                </div>
                <span class="text-[10px] px-2 py-0.5 rounded font-medium shrink-0 {voiceStatus.input_device_ok ? 'text-emerald-400 bg-emerald-500/10' : 'text-amber-400 bg-amber-500/10'}">
                  {voiceStatus.input_device_ok ? 'Подключено' : 'Переподключение...'}
                </span>
              </div>
            </div>

            <div class="pt-1 flex items-center justify-between gap-3">
              <p class="text-[11px] text-[#707482] leading-tight">
                При переподключении наушников звук подхватывается автоматически. Если звука нет — нажмите кнопку ручного обновления.
              </p>
              <button
                on:click={handleRestartAudio}
                class="py-2 px-3 bg-[#262834] hover:bg-[#323646] text-white rounded-lg text-xs font-semibold transition flex items-center gap-1.5 border border-[#3b3f52] shadow shrink-0"
              >
                <RefreshCw class="w-3.5 h-3.5 text-blue-400 {audioRestartFeedback === 'Перезапуск...' ? 'animate-spin' : ''}" />
                <span>{audioRestartFeedback || 'Обновить устройства'}</span>
              </button>
            </div>
          </div>

        </div>

        <!-- Modal Footer -->
        <div class="px-6 py-3.5 bg-[#1b1d24] border-t border-[#272a36] flex justify-end shrink-0">
          <button
            on:click={() => { showAudioSettingsModal = false; if (isMicTesting) toggleMicTest(); }}
            class="px-5 py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-xs font-semibold transition shadow"
          >
            Готово
          </button>
        </div>

      </div>
    </div>
  {/if}
</div>
