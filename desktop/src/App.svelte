<script>
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
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
    Mic,
    MicOff,
    Headphones,
    Volume2,
    VolumeX,
    Video,
    VideoOff,
    Monitor,
    MonitorOff,
    Sliders,
    Maximize2,
    X,
    User,
    PenLine,
    Paperclip,
    Download,
    Image as ImageIcon,
    Lock,
    Unlock,
  } from "lucide-svelte";

  let status = { type: "Disconnected" };
  let peers = [];
  let peerVolumes = {};
  let chatMessages = [];
  let pollInterval = null;
  let videoPollInterval = null;
  let unlistenVideo = null;
  let lastVideoFrameReceivedAt = 0;

  // Room Signaling Lifecycle state
  let signalingStatus = {
    is_open: false,
    is_waiting_first_peer: false,
    remaining_seconds: 0,
  };
  let isReopeningSignaling = false;

  function formatRemainingTime(secs) {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s < 10 ? "0" : ""}${s}`;
  }

  async function handleReopenSignaling() {
    if (isReopeningSignaling) return;
    try {
      isReopeningSignaling = true;
      await invoke("reopen_signaling");
      signalingStatus = await invoke("get_signaling_status");
    } catch (e) {
      console.error("Failed to reopen signaling:", e);
    } finally {
      isReopeningSignaling = false;
    }
  }

  // Chat input & image attachment state
  let chatInputEl;
  let fileInputEl;
  let attachedImage = null; // { thumbDataUrl, fullDataUrl, name, sizeStr }
  let lightboxImage = null; // { url, sender, time, id, loading }
  let lightboxPollTimer = null;
  let loadedImages = {}; // imageId -> "data:image/jpeg;base64," + base64

  // Modals state
  let showConnectModal = false;

  // User info
  const storedNick = localStorage.getItem("balabol_nickname");
  let nickname =
    storedNick && !storedNick.startsWith("Узел-") ? storedNick : "";
  let isEditingNick = false;
  let roomCodeInput = "";

  function getInitials(name) {
    if (!name || !name.trim()) return "??";
    const parts = name.trim().split(/\s+/);
    if (parts.length >= 2) {
      return (parts[0][0] + parts[1][0]).toUpperCase();
    }
    return name.trim().slice(0, 2).toUpperCase();
  }

  // Chat input
  let chatInput = "";
  let chatContainer;

  let copiedItem = "";
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
    },
  };

  let remoteVideoFrame = null;
  let lastReceivedFrameId = 0;
  let isCameraOn = false;
  let isScreenOn = false;
  let localStream = null;
  let localVideoEl;
  let canvasEl;
  let videoCaptureTimer = null;
  let activeTab = "split"; // 'split' | 'stage' | 'chat'

  // Audio Settings & Master Volume Controls
  let showAudioSettingsModal = false;
  let masterVolume = parseInt(
    localStorage.getItem("balabol_master_volume") || "100",
  );
  let micSensitivity = parseInt(
    localStorage.getItem("balabol_mic_sens") || "50",
  );
  let micGain = parseInt(localStorage.getItem("balabol_mic_gain") || "100");
  let isMicTesting = false;
  let audioRestartFeedback = "";

  // Anonymity & Security Mode (LAN IP concealment)
  let hideLanMode = localStorage.getItem("balabol_hide_lan") !== "false"; // default: true
  async function toggleHideLanMode() {
    hideLanMode = !hideLanMode;
    localStorage.setItem("balabol_hide_lan", hideLanMode ? "true" : "false");
    try {
      await invoke("set_hide_lan_mode", { enabled: hideLanMode });
    } catch (e) {
      console.error("Failed to set hide LAN mode:", e);
    }
  }

  // Sound Notifications (Join / Leave Audio Cues)
  let previousStatusType = "Disconnected";

  function playConnectSound() {
    try {
      const vol = (masterVolume / 100) * 0.18;
      if (vol <= 0.001) return;
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      const now = ctx.currentTime;

      // Note 1: 587.33 Hz (D5) for 120ms
      const osc1 = ctx.createOscillator();
      const gain1 = ctx.createGain();
      osc1.type = "sine";
      osc1.frequency.setValueAtTime(587.33, now);
      gain1.gain.setValueAtTime(0.001, now);
      gain1.gain.linearRampToValueAtTime(vol, now + 0.02);
      gain1.gain.exponentialRampToValueAtTime(0.001, now + 0.14);
      osc1.connect(gain1);
      gain1.connect(ctx.destination);
      osc1.start(now);
      osc1.stop(now + 0.14);

      // Note 2: 880.0 Hz (A5) for 180ms
      const osc2 = ctx.createOscillator();
      const gain2 = ctx.createGain();
      osc2.type = "sine";
      osc2.frequency.setValueAtTime(880.0, now + 0.08);
      gain2.gain.setValueAtTime(0.001, now + 0.08);
      gain2.gain.linearRampToValueAtTime(vol, now + 0.1);
      gain2.gain.exponentialRampToValueAtTime(0.001, now + 0.28);
      osc2.connect(gain2);
      gain2.connect(ctx.destination);
      osc2.start(now + 0.08);
      osc2.stop(now + 0.28);
    } catch (e) {
      console.warn("Play connect sound error:", e);
    }
  }

  function playDisconnectSound() {
    try {
      const vol = (masterVolume / 100) * 0.18;
      if (vol <= 0.001) return;
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      const now = ctx.currentTime;

      // Note 1: 659.25 Hz (E5) for 120ms
      const osc1 = ctx.createOscillator();
      const gain1 = ctx.createGain();
      osc1.type = "sine";
      osc1.frequency.setValueAtTime(659.25, now);
      gain1.gain.setValueAtTime(0.001, now);
      gain1.gain.linearRampToValueAtTime(vol, now + 0.02);
      gain1.gain.exponentialRampToValueAtTime(0.001, now + 0.14);
      osc1.connect(gain1);
      gain1.connect(ctx.destination);
      osc1.start(now);
      osc1.stop(now + 0.14);

      // Note 2: 440.0 Hz (A4) for 200ms
      const osc2 = ctx.createOscillator();
      const gain2 = ctx.createGain();
      osc2.type = "sine";
      osc2.frequency.setValueAtTime(440.0, now + 0.08);
      gain2.gain.setValueAtTime(0.001, now + 0.08);
      gain2.gain.linearRampToValueAtTime(vol, now + 0.1);
      gain2.gain.exponentialRampToValueAtTime(0.001, now + 0.32);
      osc2.connect(gain2);
      gain2.connect(ctx.destination);
      osc2.start(now + 0.08);
      osc2.stop(now + 0.32);
    } catch (e) {
      console.warn("Play disconnect sound error:", e);
    }
  }

  onMount(async () => {
    try {
      await invoke("set_master_volume", {
        volume: masterVolume / 100,
      });
      await invoke("set_mic_sensitivity", {
        sensitivity: micSensitivity / 100,
      });
      await invoke("set_mic_gain", { gain: micGain / 100 });
      await invoke("set_hide_lan_mode", { enabled: hideLanMode });
    } catch (e) {
      console.error("Audio/security parameters init error:", e);
    }

    await pollState();
    // High-responsiveness state poll (150ms) for instant chat, status & speaking indicators
    pollInterval = setInterval(pollState, 150);
    // Direct memory frame poll (60ms) for ultra-fast, stutter-free video and screen share rendering
    videoPollInterval = setInterval(pollVideoFrame, 60);

    try {
      unlistenVideo = await listen("video-frame", (event) => {
        if (event && event.payload) {
          remoteVideoFrame = event.payload;
          lastReceivedFrameId = event.payload.frame_id;
          lastVideoFrameReceivedAt = Date.now();
        }
      });
    } catch (e) {
      // Fallback: pollVideoFrame handles frames directly via memory buffer
    }
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
    if (videoPollInterval) clearInterval(videoPollInterval);
    if (unlistenVideo) unlistenVideo();
    if (isMicTesting) {
      invoke("stop_mic_test").catch(() => {});
    }
    if (lightboxPollTimer) clearInterval(lightboxPollTimer);
    stopMediaStream();
  });

  async function pollVideoFrame() {
    if (status.type === "Connected") {
      try {
        const frame = await invoke("get_remote_video_frame", {
          lastFrameId: lastReceivedFrameId,
        });
        if (frame) {
          lastReceivedFrameId = frame.frame_id;
          remoteVideoFrame = frame;
          lastVideoFrameReceivedAt = Date.now();
        } else if (
          !peers.some((p) => p.video_active || p.screen_active) &&
          !voiceStatus.remote_media?.video_active &&
          !voiceStatus.remote_media?.screen_active &&
          Date.now() - lastVideoFrameReceivedAt > 4000
        ) {
          if (remoteVideoFrame) {
            remoteVideoFrame = null;
            lastReceivedFrameId = 0;
          }
        }
      } catch (e) {}
    }
  }

  async function pollState() {
    try {
      status = await invoke("get_status");
      peers = await invoke("get_peers");

      const prevType = previousStatusType;
      previousStatusType = status.type;

      if (prevType !== "Connected" && status.type === "Connected") {
        playConnectSound();
      } else if (
        prevType === "Connected" &&
        (status.type === "Disconnected" || status.type === "Failed")
      ) {
        playDisconnectSound();
      }

      if (status.type === "Connected") {
        const newChat = await invoke("get_chat_messages");
        if (newChat.length !== chatMessages.length) {
          const lastMsg = newChat[newChat.length - 1];
          if (lastMsg && !lastMsg.is_me) {
            invoke("play_notification_sound", { soundType: "chat" }).catch(() => {});
          }
          chatMessages = newChat;
          scrollToChatBottom();
        }

        // Auto-fetch full images from P2P node for any messages missing full image
        for (const msg of newChat) {
          if (msg.image_id && !loadedImages[msg.image_id]) {
            invoke("get_cached_image", { imageId: msg.image_id }).then((b64) => {
              if (b64) {
                loadedImages = { ...loadedImages, [msg.image_id]: "data:image/jpeg;base64," + b64 };
              } else {
                invoke("request_full_image", { imageId: msg.image_id }).catch(() => {});
              }
            }).catch(() => {});
          }
        }
        // Auto-fetch signaling status
        try {
          signalingStatus = await invoke("get_signaling_status");
        } catch (e) {}
      } else {
        signalingStatus = {
          is_open: false,
          is_waiting_first_peer: false,
          remaining_seconds: 0,
        };
      }

      // Poll voice status whenever connected OR while settings modal / mic test is active
      if (
        status.type === "Connected" ||
        showAudioSettingsModal ||
        isMicTesting
      ) {
        voiceStatus = await invoke("get_voice_status");
      }
    } catch (e) {
      console.error("Poll error:", e);
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
    localStorage.setItem("balabol_nickname", name);
    if (name && name.trim()) {
      invoke("set_nickname", { nickname: name.trim() }).catch(console.error);
    }
  }

  // --- Voice Controls ---
  async function toggleMic() {
    const newMuted = !voiceStatus.mic_muted;
    await invoke("set_mic_muted", { muted: newMuted });
    voiceStatus.mic_muted = newMuted;
  }

  async function toggleDeafen() {
    const newDeafened = !voiceStatus.deafened;
    await invoke("set_deafened", { deafened: newDeafened });
    voiceStatus.deafened = newDeafened;
  }

  async function handleMasterVolumeChange(e) {
    masterVolume = parseInt(e.target.value);
    localStorage.setItem("balabol_master_volume", masterVolume.toString());
    await invoke("set_master_volume", { volume: masterVolume / 100 });
  }

  async function handleSensitivityChange(e) {
    micSensitivity = parseInt(e.target.value);
    localStorage.setItem("balabol_mic_sens", micSensitivity.toString());
    await invoke("set_mic_sensitivity", { sensitivity: micSensitivity / 100 });
  }

  async function handleGainChange(e) {
    micGain = parseInt(e.target.value);
    localStorage.setItem("balabol_mic_gain", micGain.toString());
    await invoke("set_mic_gain", { gain: micGain / 100 });
  }

  async function handleRestartAudio() {
    audioRestartFeedback = "Перезапуск...";
    try {
      await invoke("restart_audio");
      audioRestartFeedback = "Успешно обновлено!";
      setTimeout(() => {
        audioRestartFeedback = "";
      }, 2500);
    } catch (e) {
      audioRestartFeedback = "Ошибка: " + e;
      setTimeout(() => {
        audioRestartFeedback = "";
      }, 3500);
    }
  }

  async function toggleMicTest() {
    isMicTesting = !isMicTesting;
    try {
      if (isMicTesting) {
        await invoke("start_mic_test");
      } else {
        await invoke("stop_mic_test");
      }
    } catch (e) {
      console.error("Mic test error:", e);
    }
  }

  // --- Video & Screen Share Controls ---
  async function toggleCamera() {
    if (isCameraOn) {
      stopCamera();
      return;
    }
    if (isScreenOn) {
      await stopScreenShare();
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: {
          width: { ideal: 640 },
          height: { ideal: 480 },
          frameRate: { max: 15 },
        },
        audio: false,
      });
      localStream = stream;
      isCameraOn = true;
      isScreenOn = false;
      if (localVideoEl) {
        localVideoEl.srcObject = stream;
        localVideoEl.play().catch(() => {});
      }
      startCameraCapture();
      await invoke("send_media_state", {
        videoActive: true,
        screenActive: false,
      });
    } catch (e) {
      console.error("Camera error:", e);
      alert("Не удалось получить доступ к веб-камере: " + e.message);
    }
  }

  async function toggleScreenShare() {
    if (isScreenOn) {
      await stopScreenShare();
      return;
    }
    if (isCameraOn) {
      stopCamera();
    }
    try {
      await invoke("start_screen_capture");
      isScreenOn = true;
      isCameraOn = false;
    } catch (e) {
      console.error("Screen capture error:", e);
      alert("Не удалось запустить демонстрацию экрана: " + e);
    }
  }

  async function stopScreenShare() {
    if (!isScreenOn) return;
    isScreenOn = false;
    try {
      await invoke("stop_screen_capture");
    } catch (e) {
      console.error("Stop screen error:", e);
    }
  }

  function stopCamera() {
    if (videoCaptureTimer) {
      clearInterval(videoCaptureTimer);
      videoCaptureTimer = null;
    }
    if (localStream) {
      localStream.getTracks().forEach((t) => t.stop());
      localStream = null;
    }
    if (localVideoEl) {
      localVideoEl.srcObject = null;
    }
    if (isCameraOn) {
      isCameraOn = false;
      if (status.type === "Connected") {
        invoke("send_media_state", {
          videoActive: false,
          screenActive: isScreenOn,
        }).catch(() => {});
      }
    }
  }

  function stopMediaStream() {
    stopCamera();
    if (isScreenOn) {
      stopScreenShare();
    }
  }

  function startCameraCapture() {
    if (videoCaptureTimer) clearInterval(videoCaptureTimer);
    videoCaptureTimer = setInterval(captureAndSendCameraFrame, 120);
  }

  function captureAndSendCameraFrame() {
    if (!localVideoEl || !canvasEl || localVideoEl.videoWidth === 0) return;
    try {
      const targetW = 480;
      const targetH = 360;
      canvasEl.width = targetW;
      canvasEl.height = targetH;
      const ctx = canvasEl.getContext("2d");
      ctx.drawImage(localVideoEl, 0, 0, targetW, targetH);
      const dataUrl = canvasEl.toDataURL("image/jpeg", 0.4);
      const commaIdx = dataUrl.indexOf(",");
      if (commaIdx !== -1) {
        const base64 = dataUrl.substring(commaIdx + 1);
        invoke("send_video_frame", { jpegBase64: base64, isScreen: false }).catch(
          () => {},
        );
      }
    } catch (e) {
      console.error("Capture camera frame error:", e);
    }
  }

  // --- Actions: Room Code ---
  async function handleCreateRoom() {
    let currentNick = nickname.trim();
    if (!currentNick || currentNick.startsWith("Узел-")) {
      const entered = prompt(
        "Пожалуйста, введите ваше имя (никнейм) для звонка:",
        currentNick.startsWith("Узел-") ? "" : currentNick,
      );
      if (entered && entered.trim()) {
        currentNick = entered.trim();
        saveNickname(currentNick);
      } else {
        alert(
          "Имя обязательно для создания комнаты, чтобы участники понимали, кто говорит.",
        );
        return;
      }
    }

    isSubmitting = true;
    try {
      saveNickname(currentNick);
      const randomCode = Math.floor(100000 + Math.random() * 900000).toString();
      await invoke("start_room", {
        roomCode: randomCode,
        nickname: currentNick,
      });
      showConnectModal = false;
    } catch (err) {
      alert("Ошибка создания комнаты: " + err);
    } finally {
      isSubmitting = false;
    }
  }

  async function handleJoinRoom() {
    if (!roomCodeInput.trim()) {
      alert("Пожалуйста, введите 6-значный код комнаты (например, 742918)");
      return;
    }

    let currentNick = nickname.trim();
    if (!currentNick || currentNick.startsWith("Узел-")) {
      const entered = prompt(
        "Пожалуйста, введите ваше имя (никнейм) для звонка:",
        currentNick.startsWith("Узел-") ? "" : currentNick,
      );
      if (entered && entered.trim()) {
        currentNick = entered.trim();
        saveNickname(currentNick);
      } else {
        alert(
          "Имя обязательно для входа в комнату, чтобы вас узнали собеседники.",
        );
        return;
      }
    }

    isSubmitting = true;
    try {
      saveNickname(currentNick);
      await invoke("join_room", {
        roomCode: roomCodeInput.trim(),
        nickname: currentNick,
      });
      showConnectModal = false;
    } catch (err) {
      alert("Ошибка входа в комнату: " + err);
    } finally {
      isSubmitting = false;
    }
  }

  // --- Actions: Chat (Optimistic 0ms Local Update + Image Sharing) ---
  function processSelectedImage(file) {
    if (!file || !file.type.startsWith("image/")) return;
    const reader = new FileReader();
    reader.onload = (e) => {
      const rawDataUrl = e.target.result;
      const img = new Image();
      img.onload = () => {
        // 1. Full-res canvas (max 1920x1080)
        const maxFullDim = 1920;
        let fw = img.width;
        let fh = img.height;
        if (fw > maxFullDim || fh > maxFullDim) {
          if (fw > fh) {
            fh = Math.round((fh * maxFullDim) / fw);
            fw = maxFullDim;
          } else {
            fw = Math.round((fw * maxFullDim) / fh);
            fh = maxFullDim;
          }
        }
        const fullCanvas = document.createElement("canvas");
        fullCanvas.width = fw;
        fullCanvas.height = fh;
        const fctx = fullCanvas.getContext("2d");
        fctx.drawImage(img, 0, 0, fw, fh);
        const fullDataUrl = fullCanvas.toDataURL("image/jpeg", 0.85);

        // 2. Ultra-compact LQIP thumbnail canvas (max 48x48, ~300-500 bytes for instant UDP packet delivery)
        const maxThumbDim = 48;
        let tw = img.width;
        let th = img.height;
        if (tw > maxThumbDim || th > maxThumbDim) {
          if (tw > th) {
            th = Math.round((th * maxThumbDim) / tw);
            tw = maxThumbDim;
          } else {
            tw = Math.round((tw * maxThumbDim) / th);
            th = maxThumbDim;
          }
        }
        const thumbCanvas = document.createElement("canvas");
        thumbCanvas.width = tw;
        thumbCanvas.height = th;
        const tctx = thumbCanvas.getContext("2d");
        tctx.drawImage(img, 0, 0, tw, th);
        const thumbDataUrl = thumbCanvas.toDataURL("image/jpeg", 0.5);

        attachedImage = {
          thumbDataUrl,
          fullDataUrl,
          name: file.name || "Скриншот",
          sizeStr: (file.size / 1024).toFixed(1) + " KB",
        };
      };
      img.src = rawDataUrl;
    };
    reader.readAsDataURL(file);
  }

  function handleAttachClick() {
    if (fileInputEl) fileInputEl.click();
  }

  function handleFileInputChange(e) {
    const file = e.target.files?.[0];
    if (file) {
      processSelectedImage(file);
    }
    e.target.value = "";
  }

  function handleChatPaste(e) {
    const items = (e.clipboardData || window.clipboardData)?.items;
    if (!items) return;
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.kind === "file" && item.type.startsWith("image/")) {
        e.preventDefault();
        const blob = item.getAsFile();
        processSelectedImage(blob);
        return;
      }
    }
  }

  function insertPoopEmoji() {
    if (!chatInputEl) {
      chatInput += "💩";
      return;
    }
    const start = chatInputEl.selectionStart ?? chatInput.length;
    const end = chatInputEl.selectionEnd ?? chatInput.length;
    chatInput = chatInput.slice(0, start) + "💩" + chatInput.slice(end);
    setTimeout(() => {
      chatInputEl.focus();
      chatInputEl.setSelectionRange(start + 2, start + 2);
    }, 10);
  }

  async function handleSendMessage() {
    const text = chatInput.trim();
    if (!text && !attachedImage) return;

    let imgThumb = null;
    let imgId = null;
    let fullB64 = null;

    if (attachedImage) {
      imgId = "img_" + Date.now() + "_" + Math.floor(Math.random() * 100000);
      imgThumb = attachedImage.thumbDataUrl;
      const commaIdx = attachedImage.fullDataUrl.indexOf(",");
      fullB64 = commaIdx >= 0 ? attachedImage.fullDataUrl.slice(commaIdx + 1) : attachedImage.fullDataUrl;
      loadedImages = { ...loadedImages, [imgId]: attachedImage.fullDataUrl };
    }

    const textToSend = text;
    chatInput = "";
    attachedImage = null;

    // Instant optimistic UI update (0ms delay!)
    const optimisticMsg = {
      id: Date.now() + Math.random(),
      sender: nickname.trim() || "Я",
      text: textToSend,
      timestamp_ms: Date.now(),
      is_me: true,
      image_thumb: imgThumb,
      image_id: imgId,
    };
    chatMessages = [...chatMessages, optimisticMsg];
    scrollToChatBottom();

    try {
      await invoke("send_chat_message", {
        text: textToSend,
        nickname: nickname.trim() || "Я",
        imageThumb: imgThumb,
        imageId: imgId,
        fullImageBase64: fullB64,
      });
      const updated = await invoke("get_chat_messages");
      chatMessages = updated;
      scrollToChatBottom();
    } catch (err) {
      console.error("Send error:", err);
    }
  }

  function handleChatKeyDown(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSendMessage();
    }
  }

  async function openLightbox(msg) {
    if (!msg.image_thumb && !msg.image_id) return;

    const cachedUrl = msg.image_id ? loadedImages[msg.image_id] : null;

    lightboxImage = {
      url: cachedUrl || msg.image_thumb,
      sender: msg.sender,
      time: new Date(msg.timestamp_ms).toLocaleTimeString(),
      id: msg.image_id,
      loading: !cachedUrl,
    };

    if (cachedUrl || !msg.image_id) {
      lightboxImage.loading = false;
      return;
    }

    try {
      const cachedB64 = await invoke("get_cached_image", { imageId: msg.image_id });
      if (cachedB64) {
        const fullUrl = "data:image/jpeg;base64," + cachedB64;
        loadedImages = { ...loadedImages, [msg.image_id]: fullUrl };
        lightboxImage.url = fullUrl;
        lightboxImage.loading = false;
        return;
      }
    } catch (e) {
      console.error("Get cached image error:", e);
    }

    try {
      await invoke("request_full_image", { imageId: msg.image_id });
    } catch (e) {
      console.error("Request full image error:", e);
    }

    if (lightboxPollTimer) clearInterval(lightboxPollTimer);
    let attempts = 0;
    lightboxPollTimer = setInterval(async () => {
      attempts++;
      if (!lightboxImage || attempts > 50) {
        clearInterval(lightboxPollTimer);
        if (lightboxImage) lightboxImage.loading = false;
        return;
      }
      try {
        const cachedB64 = await invoke("get_cached_image", { imageId: msg.image_id });
        if (cachedB64) {
          clearInterval(lightboxPollTimer);
          const fullUrl = "data:image/jpeg;base64," + cachedB64;
          loadedImages = { ...loadedImages, [msg.image_id]: fullUrl };
          if (lightboxImage) {
            lightboxImage.url = fullUrl;
            lightboxImage.loading = false;
          }
        }
      } catch (e) {
        console.error("Poll full image error:", e);
      }
    }, 200);
  }

  function closeLightbox() {
    lightboxImage = null;
    if (lightboxPollTimer) {
      clearInterval(lightboxPollTimer);
      lightboxPollTimer = null;
    }
  }

  function downloadCurrentImage() {
    if (!lightboxImage?.url) return;
    const a = document.createElement("a");
    a.href = lightboxImage.url;
    a.download = `balabol_${lightboxImage.id || "image"}.jpg`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
  }

  function handleWindowKeyDown(e) {
    if (e.key === "Escape" && lightboxImage) {
      closeLightbox();
    }
  }

  async function handleDisconnect() {
    stopMediaStream();
    remoteVideoFrame = null;
    lastReceivedFrameId = 0;
    if (status.type === "Connected") {
      playDisconnectSound();
    }
    try {
      await invoke("disconnect");
      chatMessages = [];
    } catch (e) {
      console.error("Disconnect error:", e);
    }
  }

  function copyText(text, label) {
    navigator.clipboard.writeText(text);
    copiedItem = label;
    setTimeout(() => {
      copiedItem = "";
    }, 2000);
  }
</script>

<svelte:window on:keydown={handleWindowKeyDown} />

<!-- Hidden Offscreen Canvas for Video Frame Encoding -->
<canvas bind:this={canvasEl} class="hidden"></canvas>

<div
  class="flex flex-col h-screen bg-[#111215] text-[#e0e2e6] font-sans select-none overflow-hidden"
>
  <!-- Top TeamSpeak-style Header Bar -->
  <header
    class="h-14 bg-[#181a1f] border-b border-[#26282e] flex items-center justify-between px-5 shrink-0 shadow-md"
  >
    <div class="flex items-center gap-3">
      <div
        class="w-9 h-9 rounded-lg bg-gradient-to-tr from-blue-600 to-indigo-500 flex items-center justify-center shadow-lg shadow-blue-500/20"
      >
        <Radio class="w-5 h-5 text-white" />
      </div>
      <div>
        <div class="flex items-center gap-2">
          <span class="font-bold text-base tracking-wide text-white"
            >Balabol P2P</span
          >
          <span
            class="text-[10px] uppercase font-semibold tracking-wider bg-blue-500/10 text-blue-400 border border-blue-500/20 px-1.5 py-0.5 rounded"
            >Голос + Видео</span
          >
        </div>
        <div class="text-xs text-[#8a8e98] flex items-center gap-1.5">
          <span>Вы:</span>
          {#if isEditingNick}
            <input
              type="text"
              bind:value={nickname}
              on:blur={() => {
                isEditingNick = false;
                saveNickname(nickname);
              }}
              on:keydown={(e) => {
                if (e.key === "Enter") {
                  isEditingNick = false;
                  saveNickname(nickname);
                }
              }}
              class="bg-[#101116] border border-blue-500 rounded px-1.5 py-0.5 text-white font-medium text-xs focus:outline-none w-28"
            />
          {:else}
            <button
              on:click={() => (isEditingNick = true)}
              class="text-white font-semibold hover:text-blue-400 flex items-center gap-1 group transition cursor-pointer"
              title="Нажмите, чтобы изменить ваше отображаемое имя"
            >
              <span>{nickname || "Указать имя"}</span>
              <PenLine
                class="w-3 h-3 text-[#606470] group-hover:text-blue-400 transition"
              />
            </button>
          {/if}
          {#if voiceStatus.local_speaking}
            <span
              class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse ml-1"
              title="Микрофон передает голос"
            ></span>
          {/if}
        </div>
      </div>
    </div>

    <!-- Right Controls / Status -->
    <div class="flex items-center gap-3">
      {#if status.type === "Disconnected"}
        <div
          class="flex items-center gap-2 px-3 py-1 rounded-full bg-red-500/10 text-red-400 border border-red-500/20 text-xs font-medium"
        >
          <div class="w-2 h-2 rounded-full bg-red-500"></div>
          Отключен
        </div>
        <button
          on:click={() => (showConnectModal = true)}
          class="flex items-center gap-1.5 px-3.5 py-1.5 bg-blue-600 hover:bg-blue-500 text-white rounded-md text-xs font-semibold transition shadow-md shadow-blue-600/20"
        >
          <Server class="w-3.5 h-3.5" />
          Подключение
        </button>
      {:else if status.type === "Hosting"}
        <div
          class="flex items-center gap-2 px-3 py-1 rounded-full bg-purple-500/10 text-purple-300 border border-purple-500/20 text-xs font-medium"
        >
          <div class="w-2 h-2 rounded-full bg-purple-400 animate-pulse"></div>
          Комната:<span class="font-mono font-bold text-white tracking-widest"
            >{status.room_code || ""}</span
          >
        </div>
        <button
          on:click={handleDisconnect}
          class="flex items-center gap-1.5 px-3 py-1.5 bg-red-600/80 hover:bg-red-600 text-white rounded-md text-xs font-semibold transition shadow-md shadow-red-600/20"
        >
          <PhoneOff class="w-3.5 h-3.5" />
          Закрыть комнату
        </button>
      {:else if status.type === "Connecting"}
        <div
          class="flex items-center gap-2 px-3 py-1 rounded-full bg-amber-500/10 text-amber-300 border border-amber-500/20 text-xs font-medium"
        >
          <RefreshCw class="w-3 h-3 text-amber-400 animate-spin" />
          {status.mode || "Подключение..."}
        </div>
        <button
          on:click={handleDisconnect}
          class="px-3 py-1.5 bg-[#2b2d35] hover:bg-[#343740] text-white rounded-md text-xs font-semibold transition border border-[#3b3e48]"
        >
          Отмена
        </button>
      {:else if status.type === "Connected"}
        <!-- Active Call Tabs -->
        <div
          class="flex items-center bg-[#101116] p-1 rounded-lg border border-[#2a2d36] text-xs"
        >
          <button
            on:click={() => (activeTab = "stage")}
            class="px-3 py-1 rounded-md font-medium transition {activeTab ===
            'stage'
              ? 'bg-blue-600 text-white shadow'
              : 'text-[#8a8e98] hover:text-white'}"
          >
            Сцена
          </button>
          <button
            on:click={() => (activeTab = "split")}
            class="px-3 py-1 rounded-md font-medium transition {activeTab ===
            'split'
              ? 'bg-blue-600 text-white shadow'
              : 'text-[#8a8e98] hover:text-white'}"
          >
            Сплит
          </button>
          <button
            on:click={() => (activeTab = "chat")}
            class="px-3 py-1 rounded-md font-medium transition {activeTab ===
            'chat'
              ? 'bg-blue-600 text-white shadow'
              : 'text-[#8a8e98] hover:text-white'}"
          >
            Чат
          </button>
        </div>

        <!-- Participants Counter and Host Badge -->
        <div
          class="flex items-center gap-2 px-3 py-1.5 rounded-xl bg-[#181a22] border border-[#262834] text-xs"
        >
          <Users class="w-3.5 h-3.5 text-blue-400 shrink-0" />
          <span class="font-medium text-white">{peers.length + 1} в звонке</span
          >
          <span class="text-[#454856]">•</span>
          <span
            class="bg-blue-500/15 text-blue-300 border border-blue-500/30 px-2 py-0.5 rounded-full text-[11px] font-medium flex items-center gap-1 shadow-sm"
          >
            ⚡ Full Mesh P2P
          </span>
          {#if status.ping_ms}
            <span
              class="text-[11px] bg-emerald-950/60 px-1.5 py-0.5 rounded text-emerald-300 border border-emerald-500/30 font-mono"
            >
              {status.ping_ms} мс
            </span>
          {/if}
        </div>
        <button
          on:click={handleDisconnect}
          class="flex items-center gap-1.5 px-3 py-1.5 bg-red-600/80 hover:bg-red-600 text-white rounded-md text-xs font-semibold transition shadow-md shadow-red-600/20"
        >
          <PhoneOff class="w-3.5 h-3.5" />
          Отключиться
        </button>
      {:else if status.type === "Failed"}
        <div
          class="flex items-center gap-2 px-3 py-1 rounded-full bg-red-500/10 text-red-400 border border-red-500/20 text-xs font-medium"
        >
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
      {#if status.type === "Disconnected"}
        <!-- Disconnected Zero State -->
        <div
          class="max-w-xl mx-auto w-full my-auto flex flex-col items-center text-center"
        >
          <div
            class="w-16 h-16 rounded-2xl bg-[#1c1e24] border border-[#2a2d36] flex items-center justify-center mb-5 text-blue-400 shadow-xl"
          >
            <Radio class="w-8 h-8" />
          </div>
          <h2 class="text-2xl font-bold text-white mb-2">
            Голосовая и видео P2P связь
          </h2>
          <p class="text-sm text-[#8a8e98] mb-6 max-w-md leading-relaxed">
            Прямое соединение без серверов. Чистый нативный звук с
            VAD-сглаживанием, передача веб-камеры и рабочего стола.
          </p>

          <!-- Explicit Nickname Input Card -->
          <div
            class="w-full max-w-md mb-6 bg-[#181a22] border border-[#2b2e3c] rounded-2xl p-4 shadow-xl text-left"
          >
            <div class="flex items-center justify-between mb-2">
              <label
                for="main-nick-input"
                class="text-xs font-semibold text-[#8a8e98] flex items-center gap-1.5"
              >
                <User class="w-4 h-4 text-blue-400" />
                <span>Ваше имя в звонке:</span>
              </label>
              {#if nickname && !nickname.startsWith("Узел-")}
                <span
                  class="text-[10px] text-emerald-400 font-semibold flex items-center gap-1"
                >
                  <Check class="w-3 h-3" /> Сохранено
                </span>
              {/if}
            </div>
            <div class="relative">
              <input
                id="main-nick-input"
                type="text"
                bind:value={nickname}
                on:input={() => saveNickname(nickname)}
                placeholder="Введите имя (например: DeniskaBigSosiska, Кучерявый друг)"
                maxlength="24"
                class="w-full bg-[#101116] border border-[#2d303f] focus:border-blue-500 rounded-xl px-3.5 py-2.5 text-white font-medium text-sm placeholder-[#545766] focus:outline-none transition shadow-inner"
              />
            </div>
            <div class="text-[11px] text-[#6b6f7d] mt-1.5">
              Это имя увидят все участники конференции в чате и в списке
              говорящих.
            </div>
          </div>

          <div class="grid grid-cols-1 md:grid-cols-2 gap-4 w-full">
            <!-- Card 1: Fast Create -->
            <button
              on:click={handleCreateRoom}
              disabled={isSubmitting}
              class="flex flex-col items-start p-5 rounded-xl bg-[#1a1c22] border border-[#262832] hover:border-blue-500/40 hover:bg-[#1e2028] transition group text-left shadow-lg disabled:opacity-50"
            >
              <div
                class="w-10 h-10 rounded-lg bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400 mb-3 group-hover:scale-105 transition"
              >
                <Key class="w-5 h-5" />
              </div>
              <h3 class="text-base font-semibold text-white mb-1">
                Создать комнату
              </h3>
              <p class="text-xs text-[#8a8e98] mb-4">
                Сгенерирует 6-значный код комнаты для звонка с собеседником.
              </p>
              <div
                class="mt-auto flex items-center gap-1.5 text-xs font-semibold text-blue-400"
              >
                Получить код <ArrowRight class="w-3.5 h-3.5" />
              </div>
            </button>

            <!-- Card 2: Join by Code -->
            <div
              class="flex flex-col p-5 rounded-xl bg-[#1a1c22] border border-[#262832] text-left shadow-lg"
            >
              <div
                class="w-10 h-10 rounded-lg bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400 mb-3"
              >
                <Link class="w-5 h-5" />
              </div>
              <h3 class="text-base font-semibold text-white mb-1">
                Войти по коду
              </h3>
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
      {:else if status.type === "Hosting"}
        <!-- Hosting State -->
        <div class="max-w-2xl mx-auto w-full space-y-6 my-auto">
          <div
            class="p-6 rounded-2xl bg-gradient-to-r from-purple-950/40 to-[#1b1c24] border border-purple-500/20 shadow-xl text-center space-y-5"
          >
            <div
              class="inline-flex items-center gap-2 text-xs font-bold uppercase tracking-wider text-purple-400 px-3 py-1 rounded-full bg-purple-500/10 border border-purple-500/20"
            >
              <span class="w-2 h-2 rounded-full bg-purple-400 animate-ping"
              ></span>
              Ожидание подключения собеседника к звонку
            </div>

            <div>
              <span class="text-xs text-[#8a8e98] block mb-2"
                >Передайте этот 6-значный код собеседнику:</span
              >
              <div
                class="inline-flex items-center gap-3 bg-[#101116] border border-purple-500/40 rounded-2xl px-6 py-3 shadow-inner"
              >
                <span
                  class="text-4xl font-mono font-extrabold text-white tracking-[0.25em]"
                >
                  {status.room_code || "------"}
                </span>
                <button
                  on:click={() => copyText(status.room_code, "code")}
                  class="p-2.5 bg-purple-600 hover:bg-purple-500 text-white rounded-xl transition shadow-md"
                  title="Скопировать код"
                >
                  {#if copiedItem === "code"}
                    <Check class="w-5 h-5 text-emerald-300" />
                  {:else}
                    <Copy class="w-5 h-5" />
                  {/if}
                </button>
              </div>
              {#if copiedItem === "code"}
                <div class="text-xs text-emerald-400 font-semibold mt-2">
                  Код скопирован в буфер!
                </div>
              {/if}
            </div>

            <!-- Diagnostics -->
            <div
              class="pt-3 border-t border-[#262832] grid grid-cols-2 gap-3 text-xs text-left"
            >
              <div
                class="flex items-center gap-2 text-[#9ba0ad] bg-[#14151b] p-2.5 rounded-lg border border-[#22242e]"
              >
                <Wifi class="w-4 h-4 text-blue-400 shrink-0" />
                <div class="truncate">
                  <span class="text-[#646875] block text-[10px]">LAN IP:</span>
                  <span class="font-mono text-white text-[11px]"
                    >{status.local_addr || "Нет"}</span
                  >
                </div>
              </div>

              <div
                class="flex items-center gap-2 text-[#9ba0ad] bg-[#14151b] p-2.5 rounded-lg border border-[#22242e]"
              >
                <Globe class="w-4 h-4 text-emerald-400 shrink-0" />
                <div class="truncate">
                  <span class="text-[#646875] block text-[10px]"
                    >STUN / Публичный IP:</span
                  >
                  <span class="font-mono text-white text-[11px]"
                    >{status.public_addr || "Определяется..."}</span
                  >
                </div>
              </div>
            </div>
          </div>
        </div>
      {:else if status.type === "Connecting"}
        <!-- Connecting State -->
        <div
          class="max-w-md mx-auto w-full my-auto text-center p-8 rounded-2xl bg-[#181a20] border border-[#282b36] shadow-2xl"
        >
          <div
            class="w-16 h-16 rounded-full bg-amber-500/10 border border-amber-500/20 flex items-center justify-center mx-auto mb-5 text-amber-400"
          >
            <RefreshCw class="w-8 h-8 animate-spin" />
          </div>
          <h2 class="text-xl font-bold text-white mb-2">
            P2P Согласование и пробитие NAT
          </h2>
          <p
            class="text-xs text-amber-300 font-semibold mb-4 bg-amber-500/10 py-1.5 px-3 rounded-lg inline-block border border-amber-500/20"
          >
            {status.mode || "Встречный пробив портов..."}
          </p>
          <p class="text-xs text-[#8a8e98] mb-6">
            Цель: <span class="text-white font-semibold"
              >{status.target_str}</span
            >
          </p>
          <button
            on:click={handleDisconnect}
            class="w-full py-2.5 bg-[#262832] hover:bg-[#313440] text-white rounded-lg text-xs font-semibold transition"
          >
            Прервать подключение
          </button>
        </div>
      {:else if status.type === "Connected"}
        <!-- Connected Discord-Style Video & Audio Call Space -->
        <div class="w-full h-full flex flex-col gap-3 overflow-hidden">
          <!-- Content Area: Stage, Split, or Chat -->
          <div class="flex-1 flex gap-3 min-h-0 overflow-hidden">
            <!-- STAGE / VIDEO CALL AREA -->
            {#if activeTab === "stage" || activeTab === "split"}
              <div
                class="flex-1 relative flex flex-col bg-[#16181f] border border-[#262832] rounded-2xl overflow-hidden shadow-2xl min-w-0"
              >
                <!-- Main Stage Canvas / Video View -->
                <div
                  class="flex-1 relative bg-[#0b0c0f] flex items-center justify-center overflow-hidden"
                >
                  {#if remoteVideoFrame && (Date.now() - lastVideoFrameReceivedAt < 7000 || peers.some((p) => p.video_active || p.screen_active) || voiceStatus.remote_media?.video_active || voiceStatus.remote_media?.screen_active)}
                    <!-- Remote Video Stream -->
                    <div
                      class="relative w-full h-full flex items-center justify-center bg-black"
                    >
                      <img
                        src="data:image/jpeg;base64,{remoteVideoFrame.jpeg_base64}"
                        alt="Remote Feed"
                        class="w-full h-full object-contain select-none pointer-events-none"
                      />
                      <div
                        class="absolute top-4 left-4 bg-black/75 backdrop-blur-md px-3 py-1.5 rounded-lg border border-white/10 flex items-center gap-2 text-xs font-semibold text-white shadow-lg"
                      >
                        {#if remoteVideoFrame.is_screen}
                          <Monitor class="w-4 h-4 text-purple-400" />
                          <span>{peers.find((p) => p.id === remoteVideoFrame.sender_id)?.name || "Собеседник"} — Трансляция экрана</span>
                        {:else}
                          <Video class="w-4 h-4 text-emerald-400" />
                          <span>{peers.find((p) => p.id === remoteVideoFrame.sender_id)?.name || "Собеседник"} — Видеопоток камеры</span>
                        {/if}
                      </div>

                      <div
                        class="absolute top-4 right-4 flex items-center gap-2"
                      >
                        <button
                          on:click={() =>
                            (activeTab =
                              activeTab === "stage" ? "split" : "stage")}
                          class="bg-black/70 hover:bg-black/90 backdrop-blur-md px-3 py-1.5 rounded-lg border border-white/15 flex items-center gap-1.5 text-xs font-semibold text-white transition shadow"
                          title={activeTab === "stage"
                            ? "Вернуться в сплит"
                            : "Развернуть во весь экран"}
                        >
                          <Maximize2 class="w-3.5 h-3.5 text-blue-400" />
                          <span
                            >{activeTab === "stage"
                              ? "Сплит"
                              : "Во весь экран"}</span
                          >
                        </button>
                      </div>
                    </div>
                  {:else}
                    <!-- Multi-Participant Conference Grid (Star-SFU) -->
                    <div
                      class="w-full h-full p-5 overflow-y-auto flex flex-col items-center justify-start gap-4"
                    >
                      <!-- Prominent Room Info Bar inside the Call Area -->
                      {#if status.room_code}
                        <div
                          class="w-full max-w-5xl bg-[#181a24]/90 border border-[#2a2d3c] rounded-2xl px-5 py-3 flex items-center justify-between shadow-lg shrink-0"
                        >
                          <div class="flex items-center gap-3">
                            <div
                              class="w-9 h-9 rounded-xl bg-purple-500/10 border border-purple-500/20 flex items-center justify-center text-purple-400"
                            >
                              <Key class="w-4 h-4" />
                            </div>
                            <div>
                              <div
                                class="text-xs text-[#8a8e98] flex items-center gap-2"
                              >
                                <span>Код этой комнаты:</span>
                                <span
                                  class="font-mono font-extrabold text-white text-base tracking-widest"
                                  >{status.room_code}</span
                                >
                              </div>
                              <div class="text-[11px] text-[#6b6f7e]">
                                Передайте этот 6-значный код друзьям, чтобы
                                подключить 3+ участников.
                              </div>
                            </div>
                          </div>

                          <div class="flex items-center gap-2.5">
                            {#if signalingStatus.is_waiting_first_peer}
                              <span
                                class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-xs font-medium"
                                title="Сигналинг Nostr активен и ожидает подключения гостей"
                              >
                                <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse"></span>
                                Вход открыт (ожидание)
                              </span>
                            {:else if signalingStatus.is_open}
                              <div class="flex items-center gap-1.5">
                                <span
                                  class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-xs font-medium"
                                  title="Окно Nostr-сигналинга активно для подключения новых гостей"
                                >
                                  <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse"></span>
                                  Вход открыт: {formatRemainingTime(signalingStatus.remaining_seconds)}
                                </span>
                                <button
                                  on:click={handleReopenSignaling}
                                  disabled={isReopeningSignaling}
                                  class="px-2.5 py-1.5 bg-white/5 hover:bg-white/10 border border-white/10 rounded-xl text-xs text-gray-300 font-medium transition cursor-pointer"
                                  title="Продлить окно сигналинга еще на 5 минут"
                                >
                                  +5 мин
                                </button>
                              </div>
                            {:else}
                              <div class="flex items-center gap-1.5">
                                <span
                                  class="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-xl bg-amber-500/10 border border-amber-500/20 text-amber-400 text-xs font-medium"
                                  title="Nostr WSS релеи отключены. Новые участники не смогут зайти по коду без открытого сигналинга."
                                >
                                  <Lock class="w-3.5 h-3.5 text-amber-400" />
                                  Закрыта
                                </span>
                                <button
                                  on:click={handleReopenSignaling}
                                  disabled={isReopeningSignaling}
                                  class="px-3 py-1.5 bg-amber-500/20 hover:bg-amber-500/30 border border-amber-500/30 text-amber-300 rounded-xl text-xs font-semibold flex items-center gap-1.5 transition cursor-pointer shadow-sm"
                                  title="Запустить Nostr-сигналинг на 5 минут, чтобы впустить нового участника"
                                >
                                  <Unlock class="w-3.5 h-3.5 text-amber-300" />
                                  <span>{isReopeningSignaling ? "Открываем..." : "Впустить (+5 мин)"}</span>
                                </button>
                              </div>
                            {/if}

                            <button
                              on:click={() =>
                                copyText(status.room_code, "room_code_banner")}
                              class="px-4 py-2 bg-purple-600 hover:bg-purple-500 text-white rounded-xl text-xs font-semibold flex items-center gap-2 transition shadow-md shadow-purple-600/20 cursor-pointer"
                              title="Скопировать код комнаты в буфер обмена"
                            >
                              {#if copiedItem === "room_code_banner"}
                                <Check class="w-4 h-4 text-emerald-300" />
                                <span class="text-emerald-300 font-semibold"
                                  >Скопировано!</span
                                >
                              {:else}
                                <Copy class="w-4 h-4" />
                                <span>Скопировать код</span>
                              {/if}
                            </button>
                          </div>
                        </div>
                      {/if}

                      <div
                        class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-5 max-w-5xl w-full my-auto"
                      >
                        <!-- 1. Local Participant Card -->
                        <div
                          class="bg-[#14151b] border border-[#232632] rounded-2xl p-5 flex flex-col items-center justify-center text-center relative shadow-xl transition-all duration-200 {voiceStatus.local_speaking
                            ? 'border-emerald-500/70 shadow-emerald-500/10'
                            : ''}"
                        >

                          <div class="relative mb-3 mt-1">
                            <div
                              class="w-20 h-20 rounded-full bg-gradient-to-tr from-blue-600 to-indigo-600 flex items-center justify-center text-2xl font-bold text-white shadow-lg transition-all duration-200 {voiceStatus.local_speaking
                                ? 'ring-4 ring-emerald-500 scale-105 shadow-emerald-500/30'
                                : 'ring-2 ring-white/10'}"
                            >
                              {getInitials(nickname)}
                            </div>
                            {#if voiceStatus.local_speaking}
                              <div
                                class="absolute -top-1 -right-1 w-6 h-6 rounded-full bg-emerald-500 border-2 border-[#14151b] flex items-center justify-center shadow"
                              >
                                <Volume2 class="w-3.5 h-3.5 text-white" />
                              </div>
                            {/if}
                            {#if voiceStatus.mic_muted}
                              <div
                                class="absolute -bottom-1 -right-1 w-6 h-6 rounded-full bg-red-600 border-2 border-[#14151b] flex items-center justify-center shadow"
                              >
                                <MicOff class="w-3 h-3 text-white" />
                              </div>
                            {/if}
                          </div>

                          <div
                            class="font-bold text-sm text-white flex items-center gap-1.5"
                          >
                            <span>{nickname || "Вы"}</span>
                            <span class="text-[10px] bg-blue-500/20 text-blue-400 border border-blue-500/30 font-medium px-1.5 py-0.5 rounded-full"
                              >Вы</span
                            >
                          </div>

                          <div class="mt-2 flex items-center gap-2">
                            {#if voiceStatus.local_speaking}
                              <span
                                class="text-[10px] bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 px-2 py-0.5 rounded-full font-semibold animate-pulse"
                              >
                                Говорит
                              </span>
                            {:else if voiceStatus.mic_muted}
                              <span
                                class="text-[10px] bg-red-500/15 text-red-400 border border-red-500/20 px-2 py-0.5 rounded-full font-medium"
                              >
                                Микрофон выкл.
                              </span>
                            {:else}
                              <span
                                class="text-[10px] text-[#606472] font-medium"
                              >
                                В эфире
                              </span>
                            {/if}
                          </div>
                        </div>

                        <!-- 2. Remote Participant Cards -->
                        {#each peers as peer (peer.id)}
                          <div
                            class="bg-[#14151b] border border-[#232632] rounded-2xl p-5 flex flex-col items-center justify-center text-center relative shadow-xl transition-all duration-200 {peer.is_speaking
                              ? 'border-emerald-500/70 shadow-emerald-500/10'
                              : ''}"
                          >
                            <div class="absolute top-3 right-3 flex items-center gap-1.5">
                              <span
                                class="text-[10px] font-mono text-[#8a8e98] bg-black/40 px-1.5 py-0.5 rounded border border-white/5"
                              >
                                {peer.ping_ms} мс
                              </span>
                            </div>

                            <div class="relative mb-3 mt-1">
                              <div
                                class="w-20 h-20 rounded-full bg-gradient-to-tr from-purple-600 to-pink-600 flex items-center justify-center text-2xl font-bold text-white shadow-lg transition-all duration-200 {peer.is_speaking
                                  ? 'ring-4 ring-emerald-500 scale-105 shadow-emerald-500/30'
                                  : 'ring-2 ring-white/10'}"
                              >
                                {getInitials(peer.name)}
                              </div>
                              {#if peer.is_speaking}
                                <div
                                  class="absolute -top-1 -right-1 w-6 h-6 rounded-full bg-emerald-500 border-2 border-[#14151b] flex items-center justify-center shadow"
                                >
                                  <Volume2 class="w-3.5 h-3.5 text-white" />
                                </div>
                              {/if}
                              {#if peer.mic_muted}
                                <div
                                  class="absolute -bottom-1 -right-1 w-6 h-6 rounded-full bg-red-600 border-2 border-[#14151b] flex items-center justify-center shadow"
                                >
                                  <MicOff class="w-3 h-3 text-white" />
                                </div>
                              {/if}
                            </div>

                            
                            <div class="font-bold text-sm text-white">
                              {peer.name}
                            </div>
                            
                            <div class="mt-4 w-full px-2">
                              <div class="flex items-center justify-between text-[11px] text-[#8a8e98] mb-1.5">
                                <span>Громкость</span>
                                <span>{peerVolumes[peer.id] ?? 100}%</span>
                              </div>
                              <input
                                type="range"
                                min="0"
                                max="200"
                                step="5"
                                value={peerVolumes[peer.id] ?? 100}
                                on:input={(e) => {
                                  peerVolumes[peer.id] = parseInt(e.target.value);
                                  invoke("set_peer_volume", { peerId: peer.id, volume: peerVolumes[peer.id] / 100 });
                                }}
                                class="w-full accent-indigo-500 h-1.5 bg-[#232632] rounded-lg appearance-none cursor-pointer"
                              />
                            </div>


                            <div class="mt-2 flex items-center gap-2">
                              {#if peer.is_speaking}
                                <span
                                  class="text-[10px] bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 px-2 py-0.5 rounded-full font-semibold animate-pulse"
                                >
                                  Говорит
                                </span>
                              {:else if peer.mic_muted}
                                <span
                                  class="text-[10px] bg-red-500/15 text-red-400 border border-red-500/20 px-2 py-0.5 rounded-full font-medium"
                                >
                                  Микрофон выкл.
                                </span>
                              {:else}
                                <span
                                  class="text-[10px] text-[#606472] font-medium"
                                >
                                  Подключен
                                </span>
                              {/if}
                            </div>

                          </div>
                        {/each}

                        <!-- If no remote peers yet, show waiting card -->
                        {#if peers.length === 0}
                          <div
                            class="bg-[#14151b]/80 border border-dashed border-[#2b2e3c] rounded-2xl p-5 flex flex-col items-center justify-center text-center"
                          >
                            <div
                              class="w-12 h-12 rounded-full bg-purple-500/10 border border-purple-500/20 flex items-center justify-center text-purple-400 mb-2"
                            >
                              <Users class="w-6 h-6" />
                            </div>
                            <div class="text-xs font-semibold text-white mb-1">
                              Ожидание собеседников
                            </div>
                            <div
                              class="text-[11px] text-[#8a8e98] mb-3 max-w-[200px]"
                            >
                              Отправьте код комнаты другим участникам, чтобы они
                              присоединились.
                            </div>
                            {#if status.room_code}
                              <button
                                on:click={() =>
                                  copyText(status.room_code, "invite_code")}
                                class="px-3.5 py-1.5 bg-purple-600 hover:bg-purple-500 text-white rounded-xl text-xs font-semibold flex items-center gap-1.5 transition shadow cursor-pointer"
                              >
                                {#if copiedItem === "invite_code"}
                                  <Check class="w-3.5 h-3.5 text-emerald-300" />
                                  <span>Скопировано!</span>
                                {:else}
                                  <Copy class="w-3.5 h-3.5" />
                                  <span>Код: {status.room_code}</span>
                                {/if}
                              </button>
                            {/if}
                          </div>
                        {/if}
                      </div>
                    </div>
                  {/if}

                  <!-- Local Picture-in-Picture (PiP) Preview -->
                  <div
                    class="absolute bottom-4 right-4 z-20 transition-all {isCameraOn ||
                    isScreenOn
                      ? 'block'
                      : 'hidden'}"
                  >
                    <div
                      class="relative rounded-xl overflow-hidden border border-white/20 shadow-2xl bg-[#111215]"
                    >
                      {#if isCameraOn}
                        <video
                          bind:this={localVideoEl}
                          autoplay
                          muted
                          playsinline
                          class="w-48 h-32 object-cover"
                        ></video>
                        <div
                          class="absolute top-2 left-2 bg-black/70 backdrop-blur-sm px-2 py-0.5 rounded text-[10px] text-white font-medium flex items-center gap-1"
                        >
                          <Video class="w-3 h-3 text-blue-400" /> Вы
                        </div>
                      {:else if isScreenOn}
                        <div class="w-48 h-28 bg-[#181a24] flex flex-col items-center justify-center p-3 text-center border border-purple-500/20">
                          <Monitor class="w-7 h-7 text-purple-400 mb-1.5 animate-pulse" />
                          <span class="text-xs font-semibold text-white">Вы делитесь экраном</span>
                          <span class="text-[10px] text-[#8a8e98] mt-0.5">Трансляция активна</span>
                        </div>
                      {/if}
                    </div>
                  </div>
                </div>

                <!-- Floating Bottom Control Dock (Discord Style) -->
                <div
                  class="p-3 bg-[#13141a]/95 backdrop-blur-md border-t border-[#262832] flex items-center justify-between shrink-0"
                >
                  <!-- Left side: speaking status -->
                  <div class="flex items-center gap-3">
                    <div class="flex items-center gap-2">
                      <div
                        class="w-3 h-3 rounded-full {voiceStatus.local_speaking
                          ? 'bg-emerald-400 ring-2 ring-emerald-500/50 animate-pulse'
                          : 'bg-[#3b3e48]'}"
                      ></div>
                      <span class="text-xs text-[#8a8e98] font-medium">
                        {voiceStatus.mic_muted
                          ? "Микрофон отключен"
                          : voiceStatus.local_speaking
                            ? "Передача голоса..."
                            : "Микрофон активен"}
                      </span>
                    </div>
                  </div>

                  <!-- Center: Action Buttons -->
                  <div class="flex items-center gap-2">
                    <!-- Mic Toggle -->
                    <button
                      on:click={toggleMic}
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {voiceStatus.mic_muted
                        ? 'bg-red-600 hover:bg-red-500 text-white'
                        : voiceStatus.local_speaking
                          ? 'bg-emerald-600 hover:bg-emerald-500 text-white ring-2 ring-emerald-400'
                          : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={voiceStatus.mic_muted
                        ? "Включить микрофон"
                        : "Отключить микрофон"}
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
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {voiceStatus.deafened
                        ? 'bg-red-600 hover:bg-red-500 text-white'
                        : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={voiceStatus.deafened
                        ? "Включить звук"
                        : "Заглушить звук (дефен)"}
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
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {isCameraOn
                        ? 'bg-blue-600 hover:bg-blue-500 text-white ring-2 ring-blue-400'
                        : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={isCameraOn
                        ? "Выключить веб-камеру"
                        : "Включить веб-камеру"}
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
                      class="p-3 rounded-full transition shadow-lg flex items-center justify-center {isScreenOn
                        ? 'bg-purple-600 hover:bg-purple-500 text-white ring-2 ring-purple-400'
                        : 'bg-[#262832] hover:bg-[#323644] text-white'}"
                      title={isScreenOn
                        ? "Остановить демонстрацию"
                        : "Поделиться экраном"}
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
                      on:click={() => (showAudioSettingsModal = true)}
                      class="p-2.5 rounded-lg bg-[#22242e] hover:bg-[#2c2f3c] text-white text-xs flex items-center gap-1.5 transition"
                      title="Настройки звука и микрофона"
                    >
                      <Sliders class="w-4 h-4 text-emerald-400" />
                    </button>
                  </div>
                </div>
              </div>
            {/if}

            <!-- P2P TEXT CHAT -->
            {#if activeTab === "chat" || activeTab === "split"}
              <div
                class="{activeTab === 'split'
                  ? 'w-80 md:w-96'
                  : 'flex-1'} flex flex-col bg-[#16181f] border border-[#262832] rounded-2xl overflow-hidden shadow-2xl shrink-0"
              >
                <!-- Chat Header -->
                <div
                  class="h-11 bg-[#1a1c24] border-b border-[#262832] px-4 flex items-center justify-between text-xs font-semibold text-[#8a8e98]"
                >
                  <div class="flex items-center gap-2 text-white">
                    <MessageSquare class="w-4 h-4 text-blue-400" />
                    <span>P2P Текстовый чат</span>
                  </div>
                  <span class="text-[11px] text-[#606470]"
                    >{chatMessages.length} сообщ.</span
                  >
                </div>

                <!-- Messages Stream -->
                <div
                  bind:this={chatContainer}
                  class="flex-1 p-3 overflow-y-auto space-y-2.5 font-sans select-text"
                >
                  {#if chatMessages.length === 0}
                    <div
                      class="h-full flex flex-col items-center justify-center text-center text-xs text-[#606470] space-y-2"
                    >
                      <MessageSquare class="w-8 h-8 text-[#303340]" />
                      <span>Канал связи открыт! Напишите сообщение.</span>
                    </div>
                  {:else}
                    {#each chatMessages as msg (msg.id + "-" + (msg.is_me ? "me" : "peer") + "-" + msg.timestamp_ms)}
                      <div
                        class="flex flex-col {msg.is_me
                          ? 'items-end'
                          : 'items-start'}"
                      >
                        <div
                          class="flex items-center gap-1.5 mb-1 px-1 text-[10px] text-[#707482]"
                        >
                          <span class="font-semibold text-white"
                            >{msg.sender}</span
                          >
                          <span>•</span>
                          <span
                            >{new Date(
                              msg.timestamp_ms,
                            ).toLocaleTimeString()}</span
                          >
                        </div>
                        <div
                          class="max-w-[85%] px-3 py-1.5 rounded-2xl text-xs leading-relaxed shadow-md break-words {msg.is_me
                            ? 'bg-blue-600 text-white rounded-tr-none'
                            : 'bg-[#222530] text-[#e0e2e8] border border-[#2d303e] rounded-tl-none'}"
                        >
                          {#if msg.image_thumb || msg.image_id}
                            <button
                              type="button"
                              class="my-1 block relative group cursor-pointer overflow-hidden rounded-xl border border-white/10 text-left"
                              on:click={() => openLightbox(msg)}
                              title="Нажмите для увеличения"
                            >
                              <img
                                src={loadedImages[msg.image_id] || msg.image_thumb}
                                alt="Изображение"
                                class="max-h-56 max-w-full rounded-xl object-contain bg-black/40 transition group-hover:scale-[1.02]"
                                loading="lazy"
                              />
                              <div
                                class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition flex items-center justify-center gap-1.5 text-[11px] font-medium text-white"
                              >
                                <Maximize2 class="w-3.5 h-3.5 text-blue-400" />
                                <span>Увеличить</span>
                              </div>
                            </button>
                          {/if}
                          {#if msg.text}
                            <div class="{msg.image_thumb || msg.image_id ? 'mt-1' : ''}">
                              {msg.text}
                            </div>
                          {/if}
                        </div>
                      </div>
                    {/each}
                  {/if}
                </div>

                <!-- Chat Input Area -->
                <div class="bg-[#13141a] border-t border-[#262832] flex flex-col">
                  <!-- Attached Image Preview Chip -->
                  {#if attachedImage}
                    <div class="px-3 py-2 bg-[#1b1d26] border-b border-[#262832] flex items-center justify-between text-xs animate-fadeIn">
                      <div class="flex items-center gap-2.5 overflow-hidden">
                        <img
                          src={attachedImage.fullDataUrl || attachedImage.thumbDataUrl}
                          alt="Превью вложения"
                          class="w-10 h-10 object-cover rounded-lg border border-white/10 shrink-0"
                        />
                        <div class="truncate">
                          <div class="font-semibold text-white truncate max-w-[200px]">{attachedImage.name}</div>
                          <div class="text-[10px] text-[#707482]">{attachedImage.sizeStr} • Готово к отправке</div>
                        </div>
                      </div>
                      <button
                        type="button"
                        on:click={() => (attachedImage = null)}
                        class="p-1 hover:bg-white/10 rounded-full text-[#8a8e98] hover:text-white transition cursor-pointer"
                        title="Удалить прикрепленное изображение"
                      >
                        <X class="w-4 h-4" />
                      </button>
                    </div>
                  {/if}

                  <div class="p-2.5 flex items-center gap-1.5">
                    <!-- Hidden file input for images -->
                    <input
                      type="file"
                      accept="image/*"
                      bind:this={fileInputEl}
                      on:change={handleFileInputChange}
                      class="hidden"
                    />

                    <!-- Paperclip Button for picking images -->
                    <button
                      type="button"
                      on:click={handleAttachClick}
                      class="p-2 rounded-xl bg-[#1e202a] hover:bg-[#282b38] text-[#9a9ea8] hover:text-white transition cursor-pointer shrink-0"
                      title="Выбрать изображение с диска (📎)"
                    >
                      <Paperclip class="w-4 h-4" />
                    </button>

                    <!-- Poop Emoji Button -->
                    <button
                      type="button"
                      on:click={insertPoopEmoji}
                      class="p-1 px-2 rounded-xl bg-[#1e202a] hover:bg-[#282b38] text-base hover:scale-110 active:scale-95 transition cursor-pointer shrink-0 select-none"
                      title="Вставить 💩"
                    >
                      💩
                    </button>

                    <!-- Text Input -->
                    <input
                      type="text"
                      bind:this={chatInputEl}
                      bind:value={chatInput}
                      on:keydown={handleChatKeyDown}
                      on:paste={handleChatPaste}
                      placeholder={attachedImage ? "Добавьте подпись (Enter для отправки)..." : "Сообщение..."}
                      class="flex-1 bg-[#1a1c24] border border-[#2b2d38] rounded-xl px-3 py-2 text-xs text-white placeholder-[#555866] focus:outline-none focus:border-blue-500 transition"
                    />

                    <!-- Send Button -->
                    <button
                      type="button"
                      on:click={handleSendMessage}
                      disabled={!chatInput.trim() && !attachedImage}
                      class="p-2 bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-white rounded-xl transition shadow-md shadow-blue-600/20 cursor-pointer shrink-0"
                      title="Отправить (Enter)"
                    >
                      <Send class="w-4 h-4" />
                    </button>
                  </div>
                </div>
              </div>
            {/if}
          </div>
        </div>
      {:else if status.type === "Failed"}
        <!-- Failed State -->
        <div
          class="max-w-md mx-auto w-full my-auto text-center p-8 rounded-2xl bg-[#181a20] border border-red-500/30 shadow-2xl"
        >
          <div
            class="w-16 h-16 rounded-full bg-red-500/10 border border-red-500/20 flex items-center justify-center mx-auto mb-5 text-red-400"
          >
            <AlertCircle class="w-8 h-8" />
          </div>
          <h2 class="text-xl font-bold text-white mb-2">
            Не удалось связаться
          </h2>
          <p
            class="text-xs text-red-300/80 mb-6 bg-red-950/30 p-3 rounded-lg border border-red-500/20 leading-relaxed text-left"
          >
            {status.error}
          </p>
          <div class="space-y-2">
            <button
              on:click={() => {
                handleDisconnect();
                showConnectModal = true;
              }}
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
    <div
      class="fixed inset-0 bg-black/75 backdrop-blur-sm flex items-center justify-center p-4 z-50"
    >
      <div
        class="bg-[#181a20] border border-[#2a2d38] w-full max-w-md rounded-2xl p-6 shadow-2xl space-y-5"
      >
        <div
          class="flex items-center justify-between border-b border-[#262832] pb-4"
        >
          <h3 class="text-lg font-bold text-white flex items-center gap-2">
            <Key class="w-5 h-5 text-blue-400" />
            Подключение по коду
          </h3>
          <button
            on:click={() => (showConnectModal = false)}
            class="text-[#8a8e98] hover:text-white text-lg">✕</button
          >
        </div>

        <div>
          <label
            for="modal-nick"
            class="block text-xs font-semibold text-[#8a8e98] mb-1"
            >Ваш никнейм</label
          >
          <input
            id="modal-nick"
            bind:value={nickname}
            class="w-full bg-[#101116] border border-[#282b36] rounded-lg px-3 py-2 text-xs text-white focus:outline-none focus:border-blue-500"
            placeholder="Никнейм"
          />
        </div>

        <div class="space-y-4 text-xs pt-1">
          <!-- Option A: Create -->
          <div
            class="p-4 rounded-xl bg-[#14151b] border border-[#242732] space-y-2.5"
          >
            <h4 class="font-bold text-white text-sm">Создать новую комнату</h4>
            <p class="text-[#8a8e98]">
              Нажмите, чтобы сгенерировать 6-значный код комнаты и ожидать
              гостя.
            </p>
            <button
              disabled={isSubmitting}
              on:click={handleCreateRoom}
              class="w-full py-2.5 bg-blue-600 hover:bg-blue-500 text-white rounded-lg font-semibold transition shadow-md shadow-blue-600/20 disabled:opacity-50"
            >
              {isSubmitting ? "Создание..." : "Создать комнату и получить код"}
            </button>
          </div>

          <!-- Option B: Join -->
          <div
            class="p-4 rounded-xl bg-[#14151b] border border-[#242732] space-y-2.5"
          >
            <h4 class="font-bold text-white text-sm">Войти по коду</h4>
            <div>
              <label
                for="room-code-input-modal"
                class="block text-[#8a8e98] mb-1 font-semibold"
                >6-значный код комнаты</label
              >
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
              {isSubmitting ? "Подключение..." : "Войти в комнату"}
            </button>
          </div>
        </div>

        <div class="flex justify-end pt-2">
          <button
            on:click={() => (showConnectModal = false)}
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
    <div
      class="fixed inset-0 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4 z-50"
    >
      <div
        class="bg-[#181a20] border border-[#2b2e3b] w-full max-w-lg rounded-2xl shadow-2xl overflow-hidden flex flex-col max-h-[90vh]"
      >
        <!-- Modal Header -->
        <div
          class="px-6 py-4 bg-[#1b1d24] border-b border-[#272a36] flex items-center justify-between shrink-0"
        >
          <div class="flex items-center gap-2.5 text-white font-bold text-base">
            <div
              class="w-8 h-8 rounded-lg bg-blue-500/10 border border-blue-500/20 flex items-center justify-center text-blue-400"
            >
              <Sliders class="w-4 h-4" />
            </div>
            <span>Настройки звука и микрофона</span>
          </div>
          <button
            on:click={() => {
              showAudioSettingsModal = false;
              if (isMicTesting) toggleMicTest();
            }}
            class="text-[#8a8e98] hover:text-white p-1 rounded-lg hover:bg-white/10 transition"
          >
            <X class="w-5 h-5" />
          </button>
        </div>

        <!-- Modal Body -->
        <div class="p-6 space-y-5 overflow-y-auto">
          <!-- 1. Microphone Sensitivity & Live Meter -->
          <div
            class="space-y-3 bg-[#13141a] p-4 rounded-xl border border-[#252834]"
          >
            <div class="flex items-center justify-between">
              <label
                for="mic-sens-range"
                class="text-xs font-semibold text-white flex items-center gap-2"
              >
                <Mic class="w-4 h-4 text-emerald-400" />
                Чувствительность микрофона (VAD Порог)
              </label>
              <span
                class="text-xs font-mono font-bold text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded border border-emerald-500/20"
              >
                {micSensitivity}%
              </span>
            </div>

            <p class="text-[11px] text-[#8a8e98] leading-relaxed">
              Порог активации голоса. Настройте так, чтобы фоновый шум оставался
              слева от белой отметки, а голос пересекал её в зеленую зону.
            </p>

            <!-- Live Audio Meter Bar with Threshold Indicator -->
            <div class="space-y-1.5 pt-1">
              <div
                class="relative w-full h-4 bg-[#0d0e12] rounded-full overflow-hidden border border-[#262832]"
              >
                <!-- Dynamic Level Bar -->
                <div
                  class="h-full rounded-full transition-all duration-75 {(voiceStatus.mic_level ||
                    0) *
                    100 >=
                  micSensitivity
                    ? 'bg-gradient-to-r from-emerald-500 to-green-400 shadow-lg shadow-emerald-500/50'
                    : 'bg-gradient-to-r from-[#2a2d38] to-[#3a3f50]'}"
                  style="width: {Math.min(
                    100,
                    Math.max(0, (voiceStatus.mic_level || 0) * 100),
                  )}%"
                ></div>

                <!-- Threshold Marker Pin -->
                <div
                  class="absolute top-0 bottom-0 w-1 bg-white shadow-md z-10"
                  style="left: {micSensitivity}%;"
                  title="Порог срабатывания: {micSensitivity}%"
                ></div>
              </div>

              <!-- Meter Legend & Speaking Badge -->
              <div
                class="flex items-center justify-between text-[11px] text-[#6b7080]"
              >
                <span>Тишина / Шум</span>
                <div
                  class="flex items-center gap-1.5 font-medium {(voiceStatus.mic_level ||
                    0) *
                    100 >=
                  micSensitivity
                    ? 'text-emerald-400 font-bold'
                    : 'text-[#6b7080]'}"
                >
                  <span
                    class="w-2 h-2 rounded-full {(voiceStatus.mic_level || 0) *
                      100 >=
                    micSensitivity
                      ? 'bg-emerald-400 animate-pulse'
                      : 'bg-[#3b3e48]'}"
                  ></span>
                  <span
                    >{(voiceStatus.mic_level || 0) * 100 >= micSensitivity
                      ? "Голос активен (передача)"
                      : "Ожидание голоса"}</span
                  >
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
              <div
                class="flex justify-between text-[10px] text-[#6b7080] mt-1 font-medium"
              >
                <span>0% (Высокая / шепот)</span>
                <span>50% (Стандарт)</span>
                <span>100% (Шумоподавление)</span>
              </div>
            </div>
          </div>

          <!-- 2. Mic Gain & Echo Test -->
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <!-- Mic Gain -->
            <div
              class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-2"
            >
              <div
                class="flex justify-between items-center text-xs font-semibold text-white"
              >
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
            <div
              class="bg-[#13141a] p-4 rounded-xl border border-[#252834] flex flex-col justify-between"
            >
              <div>
                <div class="text-xs font-semibold text-white mb-1">
                  Проверка микрофона
                </div>
                <p class="text-[10px] text-[#787c8a]">
                  Слушайте себя в наушниках для проверки чистоты звука и
                  громкости.
                </p>
              </div>
              <button
                on:click={toggleMicTest}
                class="mt-3 w-full py-2 px-3 rounded-lg text-xs font-semibold transition flex items-center justify-center gap-2 {isMicTesting
                  ? 'bg-amber-600 hover:bg-amber-500 text-white ring-2 ring-amber-400'
                  : 'bg-[#22242e] hover:bg-[#2c2f3c] text-white border border-[#343847]'}"
              >
                <Headphones class="w-3.5 h-3.5" />
                <span
                  >{isMicTesting
                    ? "Остановить проверку"
                    : "Слышать себя (Тест)"}</span
                >
              </button>
            </div>
          </div>


          <!-- 3. Master Volume (Общая громкость приложения 0 - 100) -->
          <div
            class="space-y-3 bg-[#13141a] p-4 rounded-xl border border-[#252834]"
          >
            <div class="flex items-center justify-between">
              <label
                for="master-vol-range"
                class="text-xs font-semibold text-white flex items-center gap-2"
              >
                <Volume2 class="w-4 h-4 text-purple-400" />
                Общая громкость приложения
              </label>
              <span
                class="text-xs font-mono font-bold text-purple-400 bg-purple-500/10 px-2 py-0.5 rounded border border-purple-500/20"
              >
                {masterVolume}%
              </span>
            </div>

            <p class="text-[11px] text-[#8a8e98] leading-relaxed">
              Регулирует громкость всех входящих голосов и звуковых сигналов приложения (0 - 100%).
            </p>

            <div class="pt-1">
              <input
                id="master-vol-range"
                type="range"
                min="0"
                max="100"
                bind:value={masterVolume}
                on:input={handleMasterVolumeChange}
                class="w-full accent-purple-500 cursor-pointer"
              />
              <div
                class="flex justify-between text-[10px] text-[#6b7080] mt-1 font-medium"
              >
                <span>0% (Без звука)</span>
                <span>50%</span>
                <span>100% (Максимум)</span>
              </div>
            </div>
          </div>

          <!-- 4. Device Status & Reconnect / Recovery -->
          <div
            class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-3"
          >
            <div
              class="text-xs font-semibold text-white flex items-center justify-between"
            >
              <span>Аудио устройства и авто-переключение</span>
              <span
                class="text-[10px] text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded border border-emerald-500/20"
              >
                Авто-восстановление активно
              </span>
            </div>

            <div class="grid grid-cols-1 gap-2 text-xs">
              <div
                class="flex items-center justify-between p-2.5 rounded-lg bg-[#0d0e12] border border-[#1f212a]"
              >
                <div class="flex items-center gap-2 truncate pr-2">
                  <Headphones class="w-4 h-4 text-purple-400 shrink-0" />
                  <span class="text-[#8a8e98] text-[11px] shrink-0"
                    >Вывод (Наушники):</span
                  >
                  <span class="text-white text-[11px] font-mono truncate"
                    >{voiceStatus.output_device_name ||
                      "По умолчанию (Windows)"}</span
                  >
                </div>
                <span
                  class="text-[10px] px-2 py-0.5 rounded font-medium shrink-0 {voiceStatus.output_device_ok
                    ? 'text-emerald-400 bg-emerald-500/10'
                    : 'text-amber-400 bg-amber-500/10'}"
                >
                  {voiceStatus.output_device_ok
                    ? "Подключено"
                    : "Переподключение..."}
                </span>
              </div>

              <div
                class="flex items-center justify-between p-2.5 rounded-lg bg-[#0d0e12] border border-[#1f212a]"
              >
                <div class="flex items-center gap-2 truncate pr-2">
                  <Mic class="w-4 h-4 text-emerald-400 shrink-0" />
                  <span class="text-[#8a8e98] text-[11px] shrink-0"
                    >Ввод (Микрофон):</span
                  >
                  <span class="text-white text-[11px] font-mono truncate"
                    >{voiceStatus.input_device_name ||
                      "По умолчанию (Windows)"}</span
                  >
                </div>
                <span
                  class="text-[10px] px-2 py-0.5 rounded font-medium shrink-0 {voiceStatus.input_device_ok
                    ? 'text-emerald-400 bg-emerald-500/10'
                    : 'text-amber-400 bg-amber-500/10'}"
                >
                  {voiceStatus.input_device_ok
                    ? "Подключено"
                    : "Переподключение..."}
                </span>
              </div>
            </div>

            <div class="pt-1 flex items-center justify-between gap-3">
              <p class="text-[11px] text-[#707482] leading-tight">
                При переподключении наушников звук подхватывается автоматически.
                Если звука нет — нажмите кнопку ручного обновления.
              </p>
              <button
                on:click={handleRestartAudio}
                class="py-2 px-3 bg-[#262834] hover:bg-[#323646] text-white rounded-lg text-xs font-semibold transition flex items-center gap-1.5 border border-[#3b3f52] shadow shrink-0"
              >
                <RefreshCw
                  class="w-3.5 h-3.5 text-blue-400 {audioRestartFeedback ===
                  'Перезапуск...'
                    ? 'animate-spin'
                    : ''}"
                />
                <span>{audioRestartFeedback || "Обновить устройства"}</span>
              </button>
            </div>
          </div>

          <!-- 5. Security & Anonymity Settings -->
          <div
            class="bg-[#13141a] p-4 rounded-xl border border-[#252834] space-y-3"
          >
            <div
              class="text-xs font-semibold text-white flex items-center justify-between"
            >
              <div class="flex items-center gap-2">
                <Lock class="w-4 h-4 text-cyan-400" />
                <span>Безопасность и анонимность</span>
              </div>
              <span
                class="text-[10px] text-cyan-400 bg-cyan-500/10 px-2 py-0.5 rounded border border-cyan-500/20"
              >
                Argon2id & X25519 PFS
              </span>
            </div>

            <div
              class="flex items-center justify-between p-3 rounded-lg bg-[#0d0e12] border border-[#1f212a]"
            >
              <div class="space-y-0.5 pr-3">
                <div class="text-xs font-semibold text-white">
                  Сокрытие локальных LAN-адресов
                </div>
                <div class="text-[11px] text-[#8a8e98] leading-relaxed">
                  Исключает передачу локальных IP-адресов (192.168.x.x, 10.x.x.x). Собеседники видят только публичный STUN-адрес.
                </div>
              </div>
              <button
                on:click={toggleHideLanMode}
                class="px-3 py-1.5 rounded-lg text-xs font-semibold transition shrink-0 {hideLanMode
                  ? 'bg-cyan-600 hover:bg-cyan-500 text-white shadow-md shadow-cyan-600/20'
                  : 'bg-[#22242e] hover:bg-[#2b2d38] text-[#8a8e98]'}"
              >
                {hideLanMode ? "Включено" : "Выключено"}
              </button>
            </div>
          </div>
        </div>

        <!-- Modal Footer -->
        <div
          class="px-6 py-3.5 bg-[#1b1d24] border-t border-[#272a36] flex justify-end shrink-0"
        >
          <button
            on:click={() => {
              showAudioSettingsModal = false;
              if (isMicTesting) toggleMicTest();
            }}
            class="px-5 py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-xs font-semibold transition shadow"
          >
            Готово
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Lightbox Modal for Fullscreen Image Viewing -->
  {#if lightboxImage}
    <div
      class="fixed inset-0 bg-black/90 backdrop-blur-md flex flex-col items-center justify-between p-4 z-50 animate-fadeIn"
      role="dialog"
      aria-modal="true"
      aria-label="Просмотр изображения"
      tabindex="-1"
      on:click|self={closeLightbox}
    >
      <!-- Top header bar -->
      <div class="w-full max-w-5xl flex items-center justify-between py-2 text-xs text-white/80">
        <div class="flex items-center gap-2">
          <span class="font-bold text-white">{lightboxImage.sender}</span>
          <span class="text-white/40">•</span>
          <span class="text-white/60">{lightboxImage.time}</span>
          {#if lightboxImage.loading}
            <span class="bg-amber-500/20 text-amber-300 border border-amber-500/30 text-[10px] px-2 py-0.5 rounded-full flex items-center gap-1.5 ml-2">
              <RefreshCw class="w-3 h-3 animate-spin" />
              <span>Загрузка оригинала...</span>
            </span>
          {/if}
        </div>
        <div class="flex items-center gap-2">
          <button
            type="button"
            on:click={downloadCurrentImage}
            class="p-2 rounded-xl bg-white/10 hover:bg-white/20 text-white flex items-center gap-1.5 px-3 transition cursor-pointer text-xs font-semibold"
            title="Скачать изображение"
          >
            <Download class="w-4 h-4 text-emerald-400" />
            <span>Скачать</span>
          </button>
          <button
            type="button"
            on:click={closeLightbox}
            class="p-2 rounded-xl bg-white/10 hover:bg-red-500/40 text-white transition cursor-pointer"
            title="Закрыть (Esc)"
          >
            <X class="w-5 h-5" />
          </button>
        </div>
      </div>

      <!-- Center image area -->
      <div
        class="flex-1 flex items-center justify-center p-2 max-w-full max-h-[82vh] overflow-hidden"
        role="presentation"
        on:click|self={closeLightbox}
      >
        <img
          src={lightboxImage.url}
          alt="Просмотр изображения"
          class="max-w-full max-h-[80vh] rounded-2xl object-contain shadow-2xl border border-white/10 transition"
        />
      </div>

      <!-- Bottom hints -->
      <div class="text-[11px] text-white/40 pb-2">
        Нажмите <kbd class="px-1.5 py-0.5 bg-white/10 rounded text-white/70 font-mono text-[10px]">Esc</kbd> или кликните за пределами изображения, чтобы закрыть
      </div>
    </div>
  {/if}
</div>
