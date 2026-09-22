<script>
  import { onMount, onDestroy } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import {
    MessageSquare,
    Users,
    Settings,
    Mic,
    MicOff,
    Headphones,
    Monitor,
    MonitorUp,
    MonitorOff,
    Plus,
    Trash2,
    Copy,
    Check,
    Volume2,
    VolumeX,
    PhoneCall,
    PhoneOff,
    Send,
    ShieldCheck,
    Wifi,
    Globe,
    RefreshCw,
    X,
    Maximize2,
    Activity,
    UserCheck,
    UserPlus,
    Sliders,
    Layers,
    Tv,
    Headset,
    LogOut,
    Sparkles,
    UserMinus
  } from 'lucide-svelte';

  // Navigation State
  let activeTab = $state('rooms'); // 'rooms' | 'friends'
  let roomViewMode = $state('voice'); // 'voice' | 'chat'
  let showMembersSidebar = $state(true);

  // User & Network State
  let userInfo = $state({ user_id: '', public_key_hex: '', is_sfu_node: false, invite_code: '' });
  let networkStatus = $state({ public_endpoint: null, local_port: 42420, is_upnp_mapped: false, nat_type: 'Определение...', is_sfu_host: false, invite_code: '' });

  // Data Collections
  let rooms = $state([]);
  let activeRoom = $state(null);
  let roomMembers = $state([]);
  let roomVoiceParticipants = $state([]);
  let messages = $state([]);
  let messageInput = $state('');
  let contacts = $state([]);
  let connectedPeers = $state([]);
  let lanPeers = $state([]);
  let peerVolumes = $state({});

  // Audio & Mic State
  let isInVoice = $state(false);
  let currentVoiceRoomId = $state(null);
  let isMuted = $state(false);
  let isDeafened = $state(false);
  let micLevel = $state(0);
  let isEchoTest = $state(false);
  let filterMode = $state('DeepFilterNet'); // 'DeepFilterNet' | 'NoiseGate' | 'Off'
  let audioDevices = $state({ inputs: [], outputs: [] });

  // Screen Sharing State
  let isScreenSharing = $state(false);
  let screenSources = $state([]);
  let screenFrame = $state(null);
  let screenStreamerId = $state(null);

  // Modals & Popups
  let showCreateRoomModal = $state(false);
  let showInviteFriendModal = $state(false);
  let showAddFriendModal = $state(false);
  let showSettingsModal = $state(false);
  let showScreenShareModal = $state(false);

  // Form inputs
  let newRoomName = $state('');
  let newRoomEmoji = $state('🎮');
  let friendInviteInput = $state('');
  let friendNameInput = $state('');
  let copiedInvite = $state(false);
  let messagesEndRef = $state(null);

  // Toast notifications
  let toasts = $state([]);
  function showToast(text, type = 'info') {
    const id = Date.now() + Math.random();
    toasts = [...toasts, { id, text, type }];
    setTimeout(() => {
      toasts = toasts.filter(t => t.id !== id);
    }, 4000);
  }

  let pollInterval = null;
  let previousContactCount = 0;
  let previousRoomCount = 0;

  onMount(async () => {
    await refreshAll();

    // Regular polling for real-time state updates (peers, new contacts, incoming invites)
    pollInterval = setInterval(async () => {
      await pollRealtimeState();
    }, 1500);
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
  });

  async function refreshAll() {
    try {
      userInfo = await invoke('get_user_info');
      networkStatus = await invoke('get_network_status');
      rooms = await invoke('list_rooms');
      previousRoomCount = rooms.length;
      contacts = await invoke('list_contacts');
      previousContactCount = contacts.length;
      connectedPeers = await invoke('get_connected_peers');
      lanPeers = await invoke('get_lan_peers');
      audioDevices = await invoke('get_audio_devices');
      isEchoTest = await invoke('is_echo_test_active');

      if (!activeRoom && rooms.length > 0) {
        await selectRoom(rooms[0]);
      }
    } catch (e) {
      console.error('Failed to refresh data:', e);
    }
  }

  async function pollRealtimeState() {
    try {
      micLevel = await invoke('get_mic_level');
      connectedPeers = await invoke('get_connected_peers');
      lanPeers = await invoke('get_lan_peers');
      networkStatus = await invoke('get_network_status');

      // Poll contacts & detect newly added contacts in real-time
      const updatedContacts = await invoke('list_contacts');
      if (updatedContacts.length > previousContactCount) {
        const newOnes = updatedContacts.slice(previousContactCount);
        for (const c of newOnes) {
          showToast(`🎉 ${c.display_name} добавлен в контакты!`, 'success');
        }
      }
      contacts = updatedContacts;
      previousContactCount = updatedContacts.length;

      // Poll rooms & detect incoming room invites
      const updatedRooms = await invoke('list_rooms');
      if (updatedRooms.length > previousRoomCount) {
        const newOnes = updatedRooms.slice(previousRoomCount);
        for (const r of newOnes) {
          showToast(`📬 Вас добавили в беседу «${r.name}»!`, 'info');
        }
      }
      rooms = updatedRooms;
      previousRoomCount = updatedRooms.length;

      // Screen frame polling
      const frame = await invoke('get_latest_screen_frame');
      screenFrame = frame;
      screenStreamerId = await invoke('get_screen_streamer_id');
      isScreenSharing = screenStreamerId === userInfo.user_id;

      // Room specific polling if in a room
      if (activeRoom) {
        roomMembers = await invoke('list_room_members', { roomId: activeRoom.id });
        roomVoiceParticipants = await invoke('get_room_voice_participants', { roomId: activeRoom.id });
        if (roomViewMode === 'chat') {
          await loadMessages(activeRoom.id);
        }
      }
    } catch (e) {
      console.warn('Realtime poll error:', e);
    }
  }

  async function selectRoom(room) {
    activeRoom = room;
    try {
      roomMembers = await invoke('list_room_members', { roomId: room.id });
      roomVoiceParticipants = await invoke('get_room_voice_participants', { roomId: room.id });
      await loadMessages(room.id);
    } catch (e) {
      console.error('Select room error:', e);
    }
  }

  async function loadMessages(roomId) {
    try {
      const channels = await invoke('list_channels_for_room', { roomId });
      const textChannel = channels.find(c => c.channel_type === 'text') || channels[0];
      if (textChannel) {
        messages = await invoke('get_messages', { channelId: textChannel.id, limit: 50 });
      }
    } catch (e) {
      console.warn('Load messages error:', e);
    }
  }

  async function handleSendMessage(e) {
    if (e) e.preventDefault();
    if (!messageInput.trim() || !activeRoom) return;

    try {
      const channels = await invoke('list_channels_for_room', { roomId: activeRoom.id });
      const textChannel = channels.find(c => c.channel_type === 'text') || channels[0];
      if (textChannel) {
        await invoke('send_message', {
          channelId: textChannel.id,
          content: messageInput.trim()
        });
        messageInput = '';
        await loadMessages(activeRoom.id);
        if (messagesEndRef) {
          messagesEndRef.scrollIntoView({ behavior: 'smooth' });
        }
      }
    } catch (e) {
      console.error('Send message error:', e);
    }
  }

  async function joinVoiceCall() {
    if (!activeRoom) return;
    try {
      await invoke('join_room_voice', { roomId: activeRoom.id });
      isInVoice = true;
      currentVoiceRoomId = activeRoom.id;
      roomVoiceParticipants = await invoke('get_room_voice_participants', { roomId: activeRoom.id });
      showToast(`🔊 Подключено к голосовому каналу «${activeRoom.name}»`, 'success');
    } catch (e) {
      console.error('Join voice error:', e);
      showToast('Ошибка подключения к голосовому: ' + e, 'error');
    }
  }

  async function leaveVoiceCall() {
    if (!currentVoiceRoomId && !activeRoom) return;
    const rId = currentVoiceRoomId || activeRoom.id;
    try {
      if (isScreenSharing) {
        await stopScreenSharing();
      }
      await invoke('leave_room_voice', { roomId: rId });
      isInVoice = false;
      currentVoiceRoomId = null;
      if (activeRoom) {
        roomVoiceParticipants = await invoke('get_room_voice_participants', { roomId: activeRoom.id });
      }
      showToast('Отключено от голосового канала', 'info');
    } catch (e) {
      console.error('Leave voice error:', e);
    }
  }

  async function toggleMute() {
    isMuted = !isMuted;
    await invoke('set_mute', { muted: isMuted });
  }

  async function toggleDeafen() {
    isDeafened = !isDeafened;
    await invoke('set_deafen', { deafened: isDeafened });
    if (isDeafened) isMuted = true;
  }

  async function handleVolumeChange(peerId, val) {
    const vol = parseFloat(val);
    peerVolumes[peerId] = vol;
    await invoke('set_user_volume', { userId: peerId, volume: vol });
  }

  function getVolume(peerId) {
    return peerVolumes[peerId] ?? 1.0;
  }

  async function copyInviteCode() {
    const code = userInfo.invite_code || networkStatus.invite_code;
    if (code) {
      await navigator.clipboard.writeText(code);
      copiedInvite = true;
      showToast('Инвайт-код скопирован в буфер обмена!', 'success');
      setTimeout(() => { copiedInvite = false; }, 2000);
    }
  }

  async function handleConnectInvite(directCode) {
    const target = directCode || friendInviteInput.trim();
    if (!target) return;
    try {
      await invoke('connect_direct', { endpointOrInvite: target });
      if (friendNameInput.trim()) {
        await invoke('add_friend', { name: friendNameInput.trim(), inviteOrId: target });
      }
      friendInviteInput = '';
      friendNameInput = '';
      showAddFriendModal = false;
      showToast('Инвайт отправлен. Ожидание ответа узла...', 'info');
      await refreshAll();
    } catch (e) {
      showToast('Ошибка подключения: ' + e, 'error');
    }
  }

  async function handleCreateRoom() {
    if (!newRoomName.trim()) return;
    try {
      const room = await invoke('create_room', {
        name: newRoomName.trim(),
        emoji: newRoomEmoji || '🎮'
      });
      newRoomName = '';
      showCreateRoomModal = false;
      rooms = await invoke('list_rooms');
      await selectRoom(room);
      showToast(`Беседа «${room.name}» создана!`, 'success');
    } catch (e) {
      showToast('Ошибка создания комнаты: ' + e, 'error');
    }
  }

  async function handleDeleteRoom(roomId, e) {
    if (e) e.stopPropagation();
    if (!confirm('Удалить эту беседу?')) return;
    try {
      if (currentVoiceRoomId === roomId) {
        await leaveVoiceCall();
      }
      await invoke('delete_room', { roomId });
      rooms = await invoke('list_rooms');
      if (activeRoom && activeRoom.id === roomId) {
        activeRoom = rooms[0] || null;
        if (activeRoom) await selectRoom(activeRoom);
      }
      showToast('Беседа удалена', 'info');
    } catch (e) {
      console.error('Delete room error:', e);
    }
  }

  async function handleInviteFriendToRoom(friendUserId) {
    if (!activeRoom) return;
    try {
      await invoke('invite_friend_to_room', {
        roomId: activeRoom.id,
        friendUserId
      });
      roomMembers = await invoke('list_room_members', { roomId: activeRoom.id });
      showToast('Друг добавлен в беседу!', 'success');
    } catch (e) {
      showToast('Ошибка добавления друга: ' + e, 'error');
    }
  }

  async function openScreenSharePicker() {
    try {
      screenSources = await invoke('list_screen_sources');
      showScreenShareModal = true;
    } catch (e) {
      showToast('Не удалось получить экраны: ' + e, 'error');
    }
  }

  async function startScreenSharing(sourceId) {
    if (!activeRoom) return;
    try {
      const channels = await invoke('list_channels_for_room', { roomId: activeRoom.id });
      const voiceChannel = channels.find(c => c.channel_type === 'voice') || channels[0];
      const channelId = voiceChannel ? voiceChannel.id : activeRoom.id;

      await invoke('start_screen_share', { channelId, sourceId });
      isScreenSharing = true;
      showScreenShareModal = false;
      roomViewMode = 'voice';
      showToast('Демонстрация экрана запущена', 'success');
    } catch (e) {
      showToast('Ошибка запуска демонстрации: ' + e, 'error');
    }
  }

  async function stopScreenSharing() {
    try {
      await invoke('stop_screen_share');
      isScreenSharing = false;
      screenFrame = null;
      screenStreamerId = null;
      showToast('Демонстрация экрана остановлена', 'info');
    } catch (e) {
      console.error('Stop screen share error:', e);
    }
  }

  async function toggleEchoTest() {
    isEchoTest = !isEchoTest;
    await invoke('set_echo_test', { enabled: isEchoTest });
  }

  async function handleFilterChange(mode) {
    filterMode = mode;
    await invoke('set_filter_mode', { mode });
  }

  function getAvatarColor(id) {
    const colors = ['bg-indigo-600', 'bg-emerald-600', 'bg-violet-600', 'bg-rose-600', 'bg-amber-600', 'bg-cyan-600'];
    let sum = 0;
    for (let i = 0; i < (id || '').length; i++) sum += id.charCodeAt(i);
    return colors[sum % colors.length];
  }

  function formatUserId(id) {
    if (!id) return '';
    return id.length > 18 ? id.slice(0, 16) + '...' : id;
  }
</script>

<div class="flex h-screen w-screen bg-discord-dark select-none text-discord-text overflow-hidden font-sans">
  
  <!-- TOAST NOTIFICATIONS POPUP CONTAINER -->
  <div class="fixed top-4 right-4 z-50 flex flex-col gap-2 max-w-sm pointer-events-none">
    {#each toasts as toast (toast.id)}
      <div class="pointer-events-auto px-4 py-3 rounded-xl shadow-2xl text-xs font-semibold flex items-center gap-2 border transition-all animate-bounce {toast.type === 'success' ? 'bg-emerald-950/90 text-emerald-200 border-emerald-500/30' : toast.type === 'error' ? 'bg-rose-950/90 text-rose-200 border-rose-500/30' : 'bg-indigo-950/90 text-indigo-200 border-indigo-500/30'} backdrop-blur-md">
        <Sparkles class="w-4 h-4 shrink-0" />
        <span>{toast.text}</span>
      </div>
    {/each}
  </div>

  <!-- 1. LEFT ACTIVITY BAR (Clean brand icon + 2 functional tabs: Rooms & Friends) -->
  <aside class="w-18 bg-[#1e1f22] flex flex-col items-center py-4 justify-between border-r border-black/20 shrink-0 z-20">
    <div class="flex flex-col items-center gap-4 w-full">
      <!-- Static Brand Logo (Balabol P2P) -->
      <div 
        title="Balabol 2.0 (P2P Mesh Gamer Voice)"
        class="w-11 h-11 rounded-2xl bg-gradient-to-tr from-discord-blurple to-indigo-600 flex items-center justify-center text-white shadow-md cursor-default transition-all"
      >
        <Headset class="w-6 h-6" />
      </div>

      <div class="w-8 h-[2px] bg-white/10 rounded-full"></div>

      <!-- Tab Button: БЕСЕДЫ (Комнаты) -->
      <button
        onclick={() => activeTab = 'rooms'}
        title="Беседы и игровые комнаты"
        class="relative w-12 h-12 rounded-2xl flex items-center justify-center transition-all duration-200 group {activeTab === 'rooms' ? 'bg-discord-blurple text-white rounded-xl shadow-lg' : 'bg-discord-sidebar text-discord-text-muted hover:text-white hover:bg-discord-hover'}"
      >
        <span class="absolute left-0 w-1 bg-white rounded-r-full transition-all duration-200 {activeTab === 'rooms' ? 'h-8' : 'h-0 group-hover:h-3'}"></span>
        <MessageSquare class="w-5 h-5" />
        {#if isInVoice}
          <span class="absolute -top-1 -right-1 w-3.5 h-3.5 bg-discord-green rounded-full ring-2 ring-[#1e1f22] animate-ping"></span>
          <span class="absolute -top-1 -right-1 w-3.5 h-3.5 bg-discord-green rounded-full ring-2 ring-[#1e1f22]"></span>
        {/if}
      </button>

      <!-- Tab Button: ДРУЗЬЯ (Контакты) -->
      <button
        onclick={() => activeTab = 'friends'}
        title="Друзья и контакты P2P"
        class="relative w-12 h-12 rounded-2xl flex items-center justify-center transition-all duration-200 group {activeTab === 'friends' ? 'bg-discord-blurple text-white rounded-xl shadow-lg' : 'bg-discord-sidebar text-discord-text-muted hover:text-white hover:bg-discord-hover'}"
      >
        <span class="absolute left-0 w-1 bg-white rounded-r-full transition-all duration-200 {activeTab === 'friends' ? 'h-8' : 'h-0 group-hover:h-3'}"></span>
        <Users class="w-5 h-5" />
        {#if contacts.length > 0}
          <span class="absolute -top-1 -right-1 min-w-4 h-4 px-1 bg-discord-green text-white text-[10px] font-bold rounded-full flex items-center justify-center ring-2 ring-[#1e1f22]">
            {contacts.length}
          </span>
        {/if}
      </button>
    </div>

    <!-- Bottom Controls: Quality dot + Settings Gear -->
    <div class="flex flex-col items-center gap-3">
      <!-- Network Quality Dot -->
      <div 
        role="button"
        tabindex="0"
        onkeydown={(e) => { if (e.key === 'Enter') showSettingsModal = true; }}
        title="Сеть: {networkStatus.nat_type} ({networkStatus.public_endpoint || 'Локальная сеть'})"
        class="w-3.5 h-3.5 rounded-full {networkStatus.public_endpoint || networkStatus.is_upnp_mapped ? 'bg-discord-green shadow-[0_0_8px_#23a55a]' : 'bg-discord-yellow'} ring-4 ring-white/5 cursor-pointer"
        onclick={() => showSettingsModal = true}
      ></div>

      <!-- Settings Button -->
      <button
        onclick={() => showSettingsModal = true}
        title="Настройки звука и P2P"
        class="w-11 h-11 rounded-2xl bg-discord-sidebar text-discord-text-muted hover:text-white hover:bg-discord-hover flex items-center justify-center transition-all duration-200 hover:rotate-45"
      >
        <Settings class="w-5 h-5" />
      </button>
    </div>
  </aside>

  <!-- 2. SECONDARY SIDEBAR: List of Rooms OR Friends Panel -->
  <aside class="w-64 bg-discord-sidebar flex flex-col justify-between shrink-0 border-r border-black/10">
    {#if activeTab === 'rooms'}
      <!-- ROOMS (Беседы) List -->
      <div class="flex flex-col flex-1 overflow-hidden">
        <div class="h-14 px-4 flex items-center justify-between border-b border-black/20 shadow-sm shrink-0">
          <div class="flex items-center gap-2 font-bold text-discord-text-header tracking-wide text-xs">
            <Layers class="w-4 h-4 text-discord-blurple" />
            <span>БЕСЕДЫ (КОМНАТЫ)</span>
          </div>
          <button
            onclick={() => showCreateRoomModal = true}
            title="Создать новую беседу"
            class="p-1.5 rounded-lg bg-white/5 hover:bg-white/10 text-discord-text-muted hover:text-white transition-all"
          >
            <Plus class="w-4 h-4" />
          </button>
        </div>

        <div class="flex-1 overflow-y-auto px-2 py-3 space-y-1">
          {#each rooms as room (room.id)}
            {@const isCallInThisRoom = currentVoiceRoomId === room.id && isInVoice}
            <div
              role="button"
              tabindex="0"
              onkeydown={(e) => { if (e.key === 'Enter') selectRoom(room); }}
              onclick={() => selectRoom(room)}
              class="group flex items-center justify-between px-3 py-2.5 rounded-xl cursor-pointer transition-all duration-150 {activeRoom?.id === room.id ? 'bg-discord-active text-white font-medium shadow-sm' : 'hover:bg-discord-hover/60 text-discord-text-muted hover:text-discord-text'}"
            >
              <div class="flex items-center gap-2.5 min-w-0">
                <span class="text-base shrink-0">{room.emoji || '🎮'}</span>
                <div class="truncate">
                  <div class="truncate text-xs font-semibold text-white">{room.name}</div>
                  {#if isCallInThisRoom}
                    <div class="text-[10px] text-discord-green font-bold flex items-center gap-1">
                      <span class="w-1.5 h-1.5 rounded-full bg-discord-green animate-pulse"></span>
                      Вы в звонке
                    </div>
                  {/if}
                </div>
              </div>
              <button
                onclick={(e) => handleDeleteRoom(room.id, e)}
                title="Удалить беседу"
                class="opacity-0 group-hover:opacity-100 p-1 rounded hover:bg-discord-red/20 hover:text-discord-red transition-all"
              >
                <Trash2 class="w-3.5 h-3.5" />
              </button>
            </div>
          {/each}

          {#if rooms.length === 0}
            <div class="text-center py-8 px-4 text-discord-text-muted text-xs">
              <p>Нет созданных бесед.</p>
              <button
                onclick={() => showCreateRoomModal = true}
                class="mt-3 px-3 py-1.5 bg-discord-blurple text-white rounded-md hover:bg-discord-blurple-hover font-medium"
              >
                + Создать беседу
              </button>
            </div>
          {/if}
        </div>
      </div>
    {:else}
      <!-- FRIENDS (Контакты & P2P) List -->
      <div class="flex flex-col flex-1 overflow-hidden">
        <div class="h-14 px-4 flex items-center justify-between border-b border-black/20 shadow-sm shrink-0">
          <div class="flex items-center gap-2 font-bold text-discord-text-header tracking-wide text-xs">
            <Users class="w-4 h-4 text-discord-green" />
            <span>ДРУЗЬЯ ({contacts.length})</span>
          </div>
          <button
            onclick={() => showAddFriendModal = true}
            title="Добавить по инвайту"
            class="px-2.5 py-1 rounded-lg bg-discord-green/20 text-discord-green hover:bg-discord-green hover:text-white transition-all text-xs flex items-center gap-1 font-semibold"
          >
            <UserPlus class="w-3.5 h-3.5" />
            <span>Добавить</span>
          </button>
        </div>

        <div class="flex-1 overflow-y-auto px-2 py-3 space-y-1">
          <!-- My Invite Quick Box -->
          <div class="p-2.5 bg-discord-dark/50 rounded-xl border border-white/5 mb-3">
            <div class="text-[10px] font-bold text-discord-text-muted mb-1 flex items-center justify-between uppercase tracking-wider">
              <span>МОЙ ИНВАЙТ-КОД</span>
              {#if copiedInvite}
                <span class="text-discord-green flex items-center gap-1"><Check class="w-3 h-3" /> Скопировано</span>
              {/if}
            </div>
            <button
              onclick={copyInviteCode}
              class="w-full text-left font-mono text-[10px] bg-black/30 p-2 rounded-lg truncate hover:bg-black/50 transition-colors border border-white/5 flex items-center justify-between group"
            >
              <span class="truncate text-discord-text">{userInfo.invite_code || networkStatus.invite_code || 'bala://...'}</span>
              <Copy class="w-3.5 h-3.5 text-discord-text-muted group-hover:text-white shrink-0 ml-1" />
            </button>
          </div>

          <!-- Contacts List -->
          <div class="text-[10px] font-bold text-discord-text-muted px-2 uppercase tracking-wider mb-1">
            СПИСОК КОНТАКТОВ ({contacts.length})
          </div>
          {#each contacts as contact (contact.user_id)}
            {@const isOnline = connectedPeers.includes(contact.user_id)}
            <div class="flex items-center justify-between p-2 rounded-xl hover:bg-discord-hover/60 group transition-colors">
              <div class="flex items-center gap-2.5 min-w-0">
                <div class="relative shrink-0">
                  <div class="w-8 h-8 rounded-full {getAvatarColor(contact.user_id)} flex items-center justify-center text-white text-xs font-bold shadow-inner">
                    {(contact.display_name || 'U').charAt(0).toUpperCase()}
                  </div>
                  <span class="absolute bottom-0 right-0 w-2.5 h-2.5 rounded-full {isOnline ? 'bg-discord-green ring-2 ring-discord-sidebar shadow-[0_0_6px_#23a55a]' : 'bg-gray-500 ring-2 ring-discord-sidebar'}"></span>
                </div>
                <div class="min-w-0">
                  <div class="text-xs font-semibold text-discord-text truncate">{contact.display_name}</div>
                  <div class="text-[10px] text-discord-text-muted truncate font-mono">{isOnline ? 'В сети' : 'Не в сети'}</div>
                </div>
              </div>
              <div class="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                  onclick={async () => { await invoke('delete_contact', { userId: contact.user_id }); refreshAll(); showToast('Контакт удален', 'info'); }}
                  title="Удалить контакт"
                  class="p-1 rounded hover:bg-discord-red/20 text-discord-red"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </button>
              </div>
            </div>
          {/each}

          {#if contacts.length === 0}
            <div class="text-center py-6 px-3 text-discord-text-muted text-xs">
              <p>У вас пока нет друзей в контактах.</p>
              <button
                onclick={() => showAddFriendModal = true}
                class="mt-2 text-discord-blurple hover:underline font-semibold"
              >
                + Добавить первого друга
              </button>
            </div>
          {/if}

          <!-- Discovered on LAN (Wi-Fi) section -->
          {#if lanPeers.length > 0}
            <div class="pt-4 pb-1">
              <div class="text-[10px] font-bold text-discord-text-muted px-2 uppercase tracking-wider mb-1 flex items-center gap-1">
                <Wifi class="w-3 h-3 text-discord-green" />
                <span>Найдено в Wi-Fi сети ({lanPeers.length})</span>
              </div>
              {#each lanPeers as peer}
                <div class="flex items-center justify-between p-2 rounded-xl bg-discord-dark/30 border border-white/5 my-1">
                  <div class="min-w-0">
                    <div class="text-xs font-semibold text-discord-text truncate">{peer.display_name}</div>
                    <div class="text-[10px] text-discord-text-muted font-mono">{peer.endpoint}</div>
                  </div>
                  <button
                    onclick={() => handleConnectInvite(`bala://${peer.public_key_hex}@${peer.endpoint}`)}
                    class="px-2 py-1 bg-discord-green/20 hover:bg-discord-green text-discord-green hover:text-white rounded-lg text-[10px] font-bold transition-all"
                  >
                    Добавить
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- USER CONTROLS PROFILE BAR AT BOTTOM -->
    <div class="h-14 bg-[#232428] px-2.5 flex items-center justify-between border-t border-black/10 shrink-0">
      <div 
        role="button"
        tabindex="0"
        onkeydown={(e) => { if (e.key === 'Enter') showSettingsModal = true; }}
        onclick={() => showSettingsModal = true}
        class="flex items-center gap-2 min-w-0 cursor-pointer hover:bg-white/5 p-1 rounded-lg transition-colors"
      >
        <div class="relative shrink-0">
          <div class="w-8 h-8 rounded-full {getAvatarColor(userInfo.user_id)} flex items-center justify-center text-white text-xs font-bold shadow-inner">
            {(userInfo.user_id || 'Y').slice(5, 7).toUpperCase() || 'Я'}
          </div>
          <span class="absolute bottom-0 right-0 w-2.5 h-2.5 bg-discord-green rounded-full ring-2 ring-[#232428]"></span>
        </div>
        <div class="min-w-0 leading-tight">
          <div class="text-xs font-bold text-white truncate">Вы (Узел)</div>
          <div class="text-[10px] text-discord-text-muted truncate font-mono">{formatUserId(userInfo.user_id)}</div>
        </div>
      </div>

      <div class="flex items-center gap-0.5">
        <button
          onclick={toggleMute}
          title={isMuted ? "Включить микрофон" : "Выключить микрофон"}
          class="p-1.5 rounded-md hover:bg-white/10 text-discord-text-muted hover:text-white transition-colors {isMuted ? 'text-discord-red hover:text-discord-red' : ''}"
        >
          {#if isMuted}<MicOff class="w-4 h-4" />{:else}<Mic class="w-4 h-4" />{/if}
        </button>

        <button
          onclick={toggleDeafen}
          title={isDeafened ? "Включить звук" : "Заглушить звук (Deafen)"}
          class="p-1.5 rounded-md hover:bg-white/10 text-discord-text-muted hover:text-white transition-colors {isDeafened ? 'text-discord-red hover:text-discord-red' : ''}"
        >
          {#if isDeafened}<Headphones class="w-4 h-4 text-discord-red" />{:else}<Headphones class="w-4 h-4" />{/if}
        </button>

        <button
          onclick={() => showSettingsModal = true}
          title="Настройки"
          class="p-1.5 rounded-md hover:bg-white/10 text-discord-text-muted hover:text-white transition-colors"
        >
          <Settings class="w-4 h-4" />
        </button>
      </div>
    </div>
  </aside>

  <!-- 3. MAIN CONTENT WORKSPACE -->
  <main class="flex-1 flex flex-col bg-[#313338] overflow-hidden min-w-0">
    {#if activeRoom}
      <!-- Room Header Navigation -->
      <header class="h-14 px-5 border-b border-black/20 flex items-center justify-between shadow-sm shrink-0 bg-[#313338] z-10">
        <div class="flex items-center gap-3 min-w-0">
          <span class="text-2xl shrink-0">{activeRoom.emoji || '🎮'}</span>
          <div class="min-w-0">
            <h2 class="font-bold text-white text-sm truncate flex items-center gap-2">
              <span>{activeRoom.name}</span>
              <span class="text-[11px] font-normal text-discord-text-muted">
                • {roomMembers.length} {roomMembers.length === 1 ? 'участник' : 'участников'}
              </span>
            </h2>
          </div>
        </div>

        <div class="flex items-center gap-3">
          <!-- Room Subview Switcher: Voice & Video vs Text Chat -->
          <div class="flex bg-discord-dark/60 p-1 rounded-xl border border-white/5">
            <button
              onclick={() => roomViewMode = 'voice'}
              class="flex items-center gap-1.5 px-3 py-1 rounded-lg text-xs font-semibold transition-all {roomViewMode === 'voice' ? 'bg-discord-blurple text-white shadow' : 'text-discord-text-muted hover:text-white'}"
            >
              <Headset class="w-3.5 h-3.5" />
              <span>Голос & Видео</span>
              {#if roomVoiceParticipants.length > 0}
                <span class="w-2 h-2 rounded-full bg-discord-green"></span>
              {/if}
            </button>
            <button
              onclick={() => roomViewMode = 'chat'}
              class="flex items-center gap-1.5 px-3 py-1 rounded-lg text-xs font-semibold transition-all {roomViewMode === 'chat' ? 'bg-discord-blurple text-white shadow' : 'text-discord-text-muted hover:text-white'}"
            >
              <MessageSquare class="w-3.5 h-3.5" />
              <span>Текстовый чат</span>
            </button>
          </div>

          <!-- Button: + Пригласить друга в беседу -->
          <button
            onclick={() => showInviteFriendModal = true}
            class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs font-semibold bg-discord-green/20 hover:bg-discord-green text-discord-green hover:text-white border border-discord-green/30 transition-all shadow-sm"
          >
            <UserPlus class="w-3.5 h-3.5" />
            <span>+ Пригласить друга</span>
          </button>

          <!-- Toggle Members Drawer -->
          <button
            onclick={() => showMembersSidebar = !showMembersSidebar}
            title={showMembersSidebar ? "Скрыть участников" : "Показать участников"}
            class="p-2 rounded-xl bg-white/5 hover:bg-white/10 text-discord-text-muted hover:text-white transition-all"
          >
            <Users class="w-4 h-4" />
          </button>
        </div>
      </header>

      <!-- Active Content Area: Voice/Video Grid or Text Chat -->
      <div class="flex-1 flex overflow-hidden relative">
        <div class="flex-1 flex flex-col overflow-hidden relative">
          {#if roomViewMode === 'voice'}
            <!-- VOICE & VIDEO CHANNEL VIEW -->
            <div class="flex-1 flex flex-col p-6 overflow-y-auto">
              <!-- Screen Sharing Stream Player (if active in this room) -->
              {#if screenFrame}
                <div class="mb-6 bg-black rounded-2xl overflow-hidden border border-white/10 shadow-2xl relative group">
                  <div class="absolute top-3 left-3 bg-black/60 backdrop-blur-md px-3 py-1.5 rounded-lg text-xs font-bold text-white flex items-center gap-2 border border-white/10 z-10">
                    <Tv class="w-4 h-4 text-discord-green animate-pulse" />
                    <span>Трансляция экрана: {screenStreamerId === userInfo.user_id ? 'Вы' : formatUserId(screenStreamerId)}</span>
                  </div>
                  <img
                    src={screenFrame}
                    alt="Live Screen Stream"
                    class="w-full h-auto max-h-[60vh] object-contain mx-auto bg-black"
                  />
                </div>
              {/if}

              <!-- Participants Grid in Voice Call -->
              {#if isInVoice && currentVoiceRoomId === activeRoom.id}
                <div class="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4 auto-rows-max">
                  <!-- Local User Tile -->
                  <div class="bg-discord-sidebar/80 rounded-2xl p-4 flex flex-col items-center justify-between border border-white/5 relative shadow-md">
                    <div class="relative my-3">
                      <div class="w-20 h-20 rounded-full {getAvatarColor(userInfo.user_id)} flex items-center justify-center text-white text-2xl font-bold transition-all duration-100 {micLevel > 0.05 && !isMuted ? 'ring-4 ring-discord-green shadow-[0_0_15px_#23a55a]' : ''}">
                        {(userInfo.user_id || 'Y').slice(5, 7).toUpperCase() || 'Я'}
                      </div>
                      {#if isMuted}
                        <span class="absolute -bottom-1 -right-1 bg-discord-red text-white p-1 rounded-full ring-2 ring-discord-sidebar">
                          <MicOff class="w-3.5 h-3.5" />
                        </span>
                      {/if}
                    </div>

                    <div class="text-center w-full mb-3">
                      <div class="font-bold text-white text-xs truncate">Вы (Локальный узел)</div>
                      <div class="text-[10px] text-discord-text-muted font-mono">{formatUserId(userInfo.user_id)}</div>
                    </div>

                    <!-- Local Mic Level Meter -->
                    <div class="w-full bg-black/30 rounded-full h-2 overflow-hidden border border-white/5">
                      <div
                        class="h-full transition-all duration-75 {micLevel > 0.4 ? 'bg-discord-yellow' : 'bg-discord-green'}"
                        style="width: {Math.min(100, Math.round(micLevel * 200))}%"
                      ></div>
                    </div>
                  </div>

                  <!-- Remote Connected Peers in THIS Voice Room -->
                  {#each roomVoiceParticipants as peerId (peerId)}
                    {#if peerId !== userInfo.user_id}
                      {@const contact = contacts.find(c => c.user_id === peerId)}
                      {@const vol = getVolume(peerId)}
                      <div class="bg-discord-sidebar/80 rounded-2xl p-4 flex flex-col items-center justify-between border border-white/5 relative shadow-md">
                        <div class="relative my-3">
                          <div class="w-20 h-20 rounded-full {getAvatarColor(peerId)} flex items-center justify-center text-white text-2xl font-bold">
                            {(contact?.display_name || 'U').charAt(0).toUpperCase()}
                          </div>
                        </div>

                        <div class="text-center w-full mb-3">
                          <div class="font-bold text-white text-xs truncate">{contact?.display_name || 'Собеседник'}</div>
                          <div class="text-[10px] text-discord-text-muted font-mono">{formatUserId(peerId)}</div>
                        </div>

                        <!-- Per-user Volume Slider (0 - 200%) -->
                        <div class="w-full bg-black/20 p-2 rounded-xl border border-white/5">
                          <div class="flex items-center justify-between text-[10px] font-semibold text-discord-text-muted mb-1">
                            <span class="flex items-center gap-1"><Volume2 class="w-3 h-3" /> Громкость</span>
                            <span class="text-white">{Math.round(vol * 100)}%</span>
                          </div>
                          <input
                            type="range"
                            min="0"
                            max="2"
                            step="0.05"
                            value={vol}
                            oninput={(e) => handleVolumeChange(peerId, e.target.value)}
                            class="w-full h-1.5 bg-discord-dark rounded-lg appearance-none cursor-pointer accent-discord-blurple"
                          />
                        </div>
                      </div>
                    {/if}
                  {/each}
                </div>
              {:else}
                <!-- Voice Lobby (When local user has NOT clicked Join Voice) -->
                <div class="flex-1 flex flex-col items-center justify-center text-center p-8">
                  <div class="w-20 h-20 rounded-3xl bg-white/5 flex items-center justify-center mb-4 text-discord-blurple border border-white/5 shadow-inner">
                    <Headset class="w-10 h-10" />
                  </div>
                  <h3 class="text-lg font-bold text-white mb-2">Голосовой канал беседы «{activeRoom.name}»</h3>
                  
                  {#if roomVoiceParticipants.length > 0}
                    <p class="text-xs text-discord-green font-semibold mb-6 flex items-center gap-2">
                      <span class="w-2 h-2 rounded-full bg-discord-green animate-ping"></span>
                      В звонке сейчас: {roomVoiceParticipants.length} {roomVoiceParticipants.length === 1 ? 'человек' : 'человека'}
                    </p>
                  {:else}
                    <p class="text-xs text-discord-text-muted mb-6 max-w-sm">
                      Сейчас в этом голосовом канале никого нет. Присоединяйтесь, чтобы начать разговор!
                    </p>
                  {/if}

                  <button
                    onclick={joinVoiceCall}
                    class="px-6 py-3 bg-discord-green hover:bg-emerald-600 text-white font-bold rounded-2xl shadow-xl flex items-center gap-2.5 transition-all text-sm active:scale-95"
                  >
                    <PhoneCall class="w-4 h-4" />
                    <span>Войти в голосовой канал</span>
                  </button>
                </div>
              {/if}
            </div>

            <!-- Bottom Floating Voice Controls (Active when in voice call) -->
            {#if isInVoice && currentVoiceRoomId === activeRoom.id}
              <div class="p-4 bg-discord-dark/95 border-t border-black/20 flex items-center justify-center gap-4 shrink-0 backdrop-blur-md">
                <!-- Mute Button -->
                <button
                  onclick={toggleMute}
                  class="p-3 rounded-2xl {isMuted ? 'bg-discord-red text-white' : 'bg-white/10 hover:bg-white/15 text-white'} transition-all shadow-md"
                  title={isMuted ? "Включить микрофон" : "Выключить микрофон"}
                >
                  {#if isMuted}<MicOff class="w-5 h-5" />{:else}<Mic class="w-5 h-5" />{/if}
                </button>

                <!-- Deafen Button -->
                <button
                  onclick={toggleDeafen}
                  class="p-3 rounded-2xl {isDeafened ? 'bg-discord-red text-white' : 'bg-white/10 hover:bg-white/15 text-white'} transition-all shadow-md"
                  title={isDeafened ? "Включить звук" : "Заглушить звук"}
                >
                  <Headphones class="w-5 h-5" />
                </button>

                <!-- Screen Share Button -->
                {#if isScreenSharing}
                  <button
                    onclick={stopScreenSharing}
                    class="px-4 py-3 rounded-2xl bg-discord-red text-white font-bold text-xs flex items-center gap-2 shadow-md hover:bg-red-600 transition-all"
                  >
                    <MonitorOff class="w-4 h-4" />
                    <span>Остановить стрим</span>
                  </button>
                {:else}
                  <button
                    onclick={openScreenSharePicker}
                    class="px-4 py-3 rounded-2xl bg-discord-blurple hover:bg-discord-blurple-hover text-white font-bold text-xs flex items-center gap-2 shadow-md transition-all"
                  >
                    <MonitorUp class="w-4 h-4" />
                    <span>Демонстрация экрана</span>
                  </button>
                {/if}

                <!-- Leave Voice Call Button -->
                <button
                  onclick={leaveVoiceCall}
                  class="px-5 py-3 rounded-2xl bg-discord-red hover:bg-red-600 text-white font-bold text-xs flex items-center gap-2 shadow-lg transition-all active:scale-95"
                  title="Покинуть звонок"
                >
                  <PhoneOff class="w-4 h-4" />
                  <span>Отключиться</span>
                </button>
              </div>
            {/if}
          {:else}
            <!-- TEXT CHAT VIEW -->
            <div class="flex-1 flex flex-col overflow-hidden">
              <div class="flex-1 overflow-y-auto p-6 space-y-4">
                {#each messages as msg (msg.id)}
                  {@const isMe = msg.author_id === userInfo.user_id}
                  {@const authorContact = contacts.find(c => c.user_id === msg.author_id)}
                  {@const authorName = isMe ? 'Вы' : (authorContact?.display_name || formatUserId(msg.author_id))}
                  <div class="flex items-start gap-3 group">
                    <div class="w-10 h-10 rounded-full {getAvatarColor(msg.author_id)} flex items-center justify-center text-white font-bold text-xs shrink-0 shadow-inner">
                      {authorName.charAt(0).toUpperCase()}
                    </div>
                    <div class="flex-1 min-w-0">
                      <div class="flex items-baseline gap-2 mb-0.5">
                        <span class="font-bold text-xs text-white {isMe ? 'text-discord-blurple' : ''}">{authorName}</span>
                        <span class="text-[10px] text-discord-text-muted">
                          {new Date(msg.timestamp_ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                        </span>
                      </div>
                      <div class="text-xs text-discord-text leading-relaxed break-words bg-black/10 p-2.5 rounded-xl border border-white/5 inline-block">
                        {msg.content}
                      </div>
                    </div>
                  </div>
                {/each}

                {#if messages.length === 0}
                  <div class="text-center py-16 text-discord-text-muted text-xs">
                    <MessageSquare class="w-8 h-8 mx-auto mb-2 opacity-40" />
                    <p>В этой беседе пока нет сообщений.</p>
                    <p class="text-[11px] mt-1 text-discord-text-muted/60">Отправьте первое сообщение ниже!</p>
                  </div>
                {/if}
                <div bind:this={messagesEndRef}></div>
              </div>

              <!-- Message Input Box -->
              <form onsubmit={handleSendMessage} class="p-4 bg-[#313338] border-t border-black/10 shrink-0">
                <div class="bg-discord-dark/70 rounded-2xl flex items-center px-4 py-2 border border-white/5 focus-within:border-discord-blurple/50 transition-colors">
                  <input
                    type="text"
                    bind:value={messageInput}
                    placeholder="Написать в #{activeRoom.name}..."
                    class="w-full bg-transparent text-xs text-white placeholder-discord-text-muted focus:outline-none"
                  />
                  <button
                    type="submit"
                    disabled={!messageInput.trim()}
                    class="ml-2 p-1.5 rounded-xl bg-discord-blurple text-white hover:bg-discord-blurple-hover disabled:opacity-40 disabled:hover:bg-discord-blurple transition-all"
                  >
                    <Send class="w-3.5 h-3.5" />
                  </button>
                </div>
              </form>
            </div>
          {/if}
        </div>

        <!-- 4. RIGHT SIDEBAR: Room Members (Участники беседы) -->
        {#if showMembersSidebar}
          <aside class="w-60 bg-discord-sidebar border-l border-black/10 flex flex-col shrink-0">
            <div class="h-14 px-4 flex items-center justify-between border-b border-black/20 shrink-0">
              <span class="text-xs font-bold text-discord-text-header uppercase tracking-wider">
                УЧАСТНИКИ ({roomMembers.length})
              </span>
              <button
                onclick={() => showInviteFriendModal = true}
                title="Пригласить друга"
                class="p-1 rounded-lg hover:bg-white/10 text-discord-text-muted hover:text-white"
              >
                <UserPlus class="w-3.5 h-3.5" />
              </button>
            </div>

            <div class="flex-1 overflow-y-auto p-2 space-y-1">
              {#each roomMembers as member (member.user_id)}
                <div class="flex items-center gap-2.5 p-2 rounded-xl hover:bg-discord-hover/60 transition-colors">
                  <div class="relative shrink-0">
                    <div class="w-7 h-7 rounded-full {getAvatarColor(member.user_id)} flex items-center justify-center text-white text-[11px] font-bold shadow-inner">
                      {(member.display_name || 'U').charAt(0).toUpperCase()}
                    </div>
                    <span class="absolute bottom-0 right-0 w-2 h-2 rounded-full {member.is_online ? 'bg-discord-green ring-1 ring-discord-sidebar' : 'bg-gray-500 ring-1 ring-discord-sidebar'}"></span>
                  </div>
                  <div class="min-w-0 flex-1">
                    <div class="text-xs font-semibold text-discord-text truncate flex items-center gap-1.5">
                      <span class="truncate">{member.display_name}</span>
                      {#if member.is_in_voice}
                        <span class="text-[10px] text-discord-green font-bold">🔊</span>
                      {/if}
                    </div>
                    <div class="text-[10px] text-discord-text-muted font-mono">{member.role === 'owner' ? 'Создатель' : 'Участник'}</div>
                  </div>
                </div>
              {/each}

              {#if roomMembers.length <= 1}
                <div class="p-3 bg-black/10 rounded-xl border border-white/5 text-center mt-3">
                  <p class="text-[11px] text-discord-text-muted mb-2">Вы пока единственный участник в этой беседе.</p>
                  <button
                    onclick={() => showInviteFriendModal = true}
                    class="w-full py-1.5 px-2 bg-discord-blurple text-white rounded-lg text-xs font-bold hover:bg-discord-blurple-hover transition-all"
                  >
                    + Добавить друга
                  </button>
                </div>
              {/if}
            </div>
          </aside>
        {/if}
      </div>
    {:else}
      <!-- Empty Room State -->
      <div class="flex-1 flex flex-col items-center justify-center text-center p-8">
        <div class="w-16 h-16 rounded-3xl bg-white/5 flex items-center justify-center mb-4 text-discord-blurple">
          <Layers class="w-8 h-8" />
        </div>
        <h3 class="text-base font-bold text-white mb-2">Выберите или создайте беседу</h3>
        <p class="text-xs text-discord-text-muted max-w-sm mb-4">
          Создавайте комнаты для общения с друзьями во время игр через защищенную P2P сеть.
        </p>
        <button
          onclick={() => showCreateRoomModal = true}
          class="px-4 py-2 bg-discord-blurple text-white rounded-xl hover:bg-discord-blurple-hover font-semibold text-xs shadow-lg"
        >
          + Создать беседу
        </button>
      </div>
    {/if}
  </main>

  <!-- ==================== MODALS ==================== -->

  <!-- 1. MODAL: INVITE FRIEND TO ACTIVE ROOM -->
  {#if showInviteFriendModal && activeRoom}
    <div class="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div class="bg-discord-sidebar w-full max-w-md rounded-2xl border border-white/10 p-6 shadow-2xl flex flex-col">
        <div class="flex items-center justify-between mb-4">
          <div class="flex items-center gap-2">
            <span class="text-xl">{activeRoom.emoji}</span>
            <h3 class="font-bold text-white text-sm">Пригласить друга в «{activeRoom.name}»</h3>
          </div>
          <button onclick={() => showInviteFriendModal = false} class="text-discord-text-muted hover:text-white">
            <X class="w-4 h-4" />
          </button>
        </div>

        <p class="text-xs text-discord-text-muted mb-4">
          Выберите друга из списка контактов. Ему будет мгновенно отправлен P2P-пакет приглашения, и беседа появится у него.
        </p>

        <div class="flex-1 max-h-60 overflow-y-auto space-y-1 mb-4">
          {#each contacts as friend (friend.user_id)}
            {@const isAlreadyMember = roomMembers.some(m => m.user_id === friend.user_id)}
            <div class="flex items-center justify-between p-2.5 rounded-xl bg-black/20 border border-white/5">
              <div class="flex items-center gap-2.5 min-w-0">
                <div class="w-8 h-8 rounded-full {getAvatarColor(friend.user_id)} flex items-center justify-center text-white text-xs font-bold">
                  {(friend.display_name || 'U').charAt(0).toUpperCase()}
                </div>
                <div class="min-w-0">
                  <div class="text-xs font-semibold text-white truncate">{friend.display_name}</div>
                  <div class="text-[10px] text-discord-text-muted font-mono">{formatUserId(friend.user_id)}</div>
                </div>
              </div>
              {#if isAlreadyMember}
                <span class="text-[11px] font-semibold text-discord-green px-2 py-1 bg-discord-green/10 rounded-lg">
                  В беседе
                </span>
              {:else}
                <button
                  onclick={() => handleInviteFriendToRoom(friend.user_id)}
                  class="px-3 py-1 bg-discord-blurple hover:bg-discord-blurple-hover text-white text-xs font-bold rounded-lg transition-all"
                >
                  Добавить
                </button>
              {/if}
            </div>
          {/each}

          {#if contacts.length === 0}
            <div class="text-center py-6 text-discord-text-muted text-xs">
              <p>У вас пока нет друзей в контактах.</p>
              <button
                onclick={() => { showInviteFriendModal = false; showAddFriendModal = true; }}
                class="mt-2 text-discord-blurple font-bold hover:underline"
              >
                + Добавить контакт по инвайт-коду
              </button>
            </div>
          {/if}
        </div>

        <button
          onclick={() => showInviteFriendModal = false}
          class="w-full py-2 bg-white/5 hover:bg-white/10 rounded-xl text-xs font-semibold text-white transition-colors"
        >
          Закрыть
        </button>
      </div>
    </div>
  {/if}

  <!-- 2. MODAL: CREATE ROOM -->
  {#if showCreateRoomModal}
    <div class="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div class="bg-discord-sidebar w-full max-w-md rounded-2xl border border-white/10 p-6 shadow-2xl">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-bold text-white text-base">Создать новую беседу</h3>
          <button onclick={() => showCreateRoomModal = false} class="text-discord-text-muted hover:text-white">
            <X class="w-4 h-4" />
          </button>
        </div>

        <div class="space-y-4">
          <div>
            <label for="room-emoji-select" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Иконка (Эмодзи)</label>
            <div id="room-emoji-select" class="flex gap-2">
              {#each ['🎮', '⚔️', '🎧', '🚀', '💬', '🍕', '🏆', '🔥'] as em}
                <button
                  type="button"
                  onclick={() => newRoomEmoji = em}
                  class="w-10 h-10 rounded-xl text-xl flex items-center justify-center transition-all {newRoomEmoji === em ? 'bg-discord-blurple ring-2 ring-white scale-110 shadow-lg' : 'bg-discord-dark hover:bg-white/10'}"
                >
                  {em}
                </button>
              {/each}
            </div>
          </div>

          <div>
            <label for="room-name-input" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Название беседы</label>
            <input
              id="room-name-input"
              type="text"
              bind:value={newRoomName}
              placeholder="Например: Рейд в CS2 или Гостиная"
              class="w-full bg-discord-dark p-3 rounded-xl border border-white/5 text-xs text-white focus:outline-none focus:border-discord-blurple"
            />
          </div>
        </div>

        <div class="mt-6 flex justify-end gap-2">
          <button
            type="button"
            onclick={() => showCreateRoomModal = false}
            class="px-4 py-2 bg-white/5 hover:bg-white/10 rounded-xl text-xs font-semibold text-white"
          >
            Отмена
          </button>
          <button
            type="button"
            onclick={handleCreateRoom}
            class="px-5 py-2 bg-discord-blurple hover:bg-discord-blurple-hover rounded-xl text-xs font-bold text-white shadow-lg"
          >
            Создать
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- 3. MODAL: ADD FRIEND / DIRECT CONNECT -->
  {#if showAddFriendModal}
    <div class="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div class="bg-discord-sidebar w-full max-w-md rounded-2xl border border-white/10 p-6 shadow-2xl">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-bold text-white text-base">Добавить друга по инвайт-коду</h3>
          <button onclick={() => showAddFriendModal = false} class="text-discord-text-muted hover:text-white">
            <X class="w-4 h-4" />
          </button>
        </div>

        <div class="space-y-4">
          <div>
            <label for="friend-name-input" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Имя друга (Опционально)</label>
            <input
              id="friend-name-input"
              type="text"
              bind:value={friendNameInput}
              placeholder="Например: Иван"
              class="w-full bg-discord-dark p-3 rounded-xl border border-white/5 text-xs text-white focus:outline-none focus:border-discord-blurple"
            />
          </div>

          <div>
            <label for="friend-invite-input" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Инвайт-код друга</label>
            <input
              id="friend-invite-input"
              type="text"
              bind:value={friendInviteInput}
              placeholder="bala://<pubkey>@<ip>:<port>"
              class="w-full bg-discord-dark p-3 rounded-xl border border-white/5 text-xs font-mono text-white focus:outline-none focus:border-discord-blurple"
            />
          </div>
        </div>

        <div class="mt-6 flex justify-end gap-2">
          <button
            type="button"
            onclick={() => showAddFriendModal = false}
            class="px-4 py-2 bg-white/5 hover:bg-white/10 rounded-xl text-xs font-semibold text-white"
          >
            Отмена
          </button>
          <button
            type="button"
            onclick={() => handleConnectInvite()}
            class="px-5 py-2 bg-discord-green hover:bg-emerald-600 rounded-xl text-xs font-bold text-white shadow-lg"
          >
            Подключиться
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- 4. MODAL: SCREEN SHARE PICKER -->
  {#if showScreenShareModal}
    <div class="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div class="bg-discord-sidebar w-full max-w-lg rounded-2xl border border-white/10 p-6 shadow-2xl">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-bold text-white text-base">Выберите экран для трансляции</h3>
          <button onclick={() => showScreenShareModal = false} class="text-discord-text-muted hover:text-white">
            <X class="w-4 h-4" />
          </button>
        </div>

        <div class="grid grid-cols-2 gap-3 max-h-72 overflow-y-auto mb-6">
          {#each screenSources as src}
            <button
              type="button"
              onclick={() => startScreenSharing(src.id)}
              class="p-4 rounded-xl bg-discord-dark hover:bg-discord-blurple/20 border border-white/5 hover:border-discord-blurple text-left transition-all group flex flex-col items-center justify-center text-center"
            >
              <Monitor class="w-10 h-10 text-discord-text-muted group-hover:text-discord-blurple mb-2" />
              <div class="text-xs font-bold text-white truncate w-full">{src.name}</div>
              <div class="text-[10px] text-discord-text-muted">{src.source_type}</div>
            </button>
          {/each}
        </div>

        <div class="flex justify-end">
          <button
            type="button"
            onclick={() => showScreenShareModal = false}
            class="px-4 py-2 bg-white/5 hover:bg-white/10 rounded-xl text-xs font-semibold text-white"
          >
            Отмена
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- 5. MODAL: SETTINGS (Audio Devices, DSP Noise Gate & Network Diagnostics) -->
  {#if showSettingsModal}
    <div class="fixed inset-0 bg-black/70 backdrop-blur-sm z-50 flex items-center justify-center p-4">
      <div class="bg-discord-sidebar w-full max-w-xl rounded-2xl border border-white/10 p-6 shadow-2xl max-h-[90vh] overflow-y-auto">
        <div class="flex items-center justify-between mb-6 pb-3 border-b border-white/10">
          <div class="flex items-center gap-2">
            <Settings class="w-5 h-5 text-discord-blurple" />
            <h3 class="font-bold text-white text-base">Настройки звука и P2P-сети</h3>
          </div>
          <button onclick={() => showSettingsModal = false} class="text-discord-text-muted hover:text-white">
            <X class="w-5 h-5" />
          </button>
        </div>

        <div class="space-y-6">
          <!-- Echo Test Section with smooth DSP Noise Gate -->
          <div class="p-4 bg-discord-dark/50 rounded-2xl border border-white/5">
            <h4 class="text-xs font-bold text-white uppercase tracking-wider mb-2 flex items-center gap-2">
              <Headphones class="w-4 h-4 text-discord-green" />
              <span>Проверка микрофона (Echo Test)</span>
            </h4>
            <p class="text-[11px] text-discord-text-muted mb-4">
              Включите тест, чтобы услышать свой голос через DSP Noise Gate с мягким шумоподавлением без скрежета и высокочастотных щелчков.
            </p>

            <div class="flex items-center gap-4">
              <button
                type="button"
                onclick={toggleEchoTest}
                class="px-4 py-2 rounded-xl text-xs font-bold transition-all shadow-md {isEchoTest ? 'bg-discord-red text-white' : 'bg-discord-green text-white hover:bg-emerald-600'}"
              >
                {isEchoTest ? 'Остановить проверку' : 'Начать проверку звука'}
              </button>

              <div class="flex-1 bg-black/40 h-3 rounded-full overflow-hidden border border-white/5">
                <div
                  class="h-full transition-all duration-75 {micLevel > 0.4 ? 'bg-discord-yellow' : 'bg-discord-green'}"
                  style="width: {Math.min(100, Math.round(micLevel * 200))}%"
                ></div>
              </div>
            </div>
          </div>

          <!-- Noise Suppression Mode Selector -->
          <div>
            <label for="filter-mode-select" class="text-xs font-bold text-discord-text-muted uppercase mb-2 block">Шумоподавление (DSP / Нейросеть)</label>
            <div id="filter-mode-select" class="grid grid-cols-3 gap-2">
              <button
                type="button"
                onclick={() => handleFilterChange('DeepFilterNet')}
                class="p-2.5 rounded-xl border text-xs font-semibold transition-all {filterMode === 'DeepFilterNet' ? 'bg-discord-blurple border-white/20 text-white shadow' : 'bg-discord-dark border-white/5 text-discord-text-muted hover:text-white'}"
              >
                RNNoise (Нейросеть)
              </button>
              <button
                type="button"
                onclick={() => handleFilterChange('NoiseGate')}
                class="p-2.5 rounded-xl border text-xs font-semibold transition-all {filterMode === 'NoiseGate' ? 'bg-discord-blurple border-white/20 text-white shadow' : 'bg-discord-dark border-white/5 text-discord-text-muted hover:text-white'}"
              >
                DSP Noise Gate
              </button>
              <button
                type="button"
                onclick={() => handleFilterChange('Off')}
                class="p-2.5 rounded-xl border text-xs font-semibold transition-all {filterMode === 'Off' ? 'bg-discord-blurple border-white/20 text-white shadow' : 'bg-discord-dark border-white/5 text-discord-text-muted hover:text-white'}"
              >
                Выключено
              </button>
            </div>
          </div>

          <!-- Audio Device Pickers -->
          <div class="space-y-3">
            <div>
              <label for="audio-input-device" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Устройство ввода (Микрофон)</label>
              <select id="audio-input-device" class="w-full bg-discord-dark p-2.5 rounded-xl border border-white/5 text-xs text-white">
                {#each audioDevices.inputs as dev}
                  <option value={dev.name}>{dev.name}</option>
                {/each}
              </select>
            </div>

            <div>
              <label for="audio-output-device" class="text-xs font-bold text-discord-text-muted uppercase mb-1.5 block">Устройство вывода (Наушники)</label>
              <select id="audio-output-device" class="w-full bg-discord-dark p-2.5 rounded-xl border border-white/5 text-xs text-white">
                {#each audioDevices.outputs as dev}
                  <option value={dev.name}>{dev.name}</option>
                {/each}
              </select>
            </div>
          </div>

          <!-- Network Diagnostics Card -->
          <div class="p-4 bg-discord-dark/50 rounded-2xl border border-white/5 text-xs space-y-2">
            <h4 class="font-bold text-white uppercase tracking-wider mb-2 flex items-center gap-1.5">
              <Globe class="w-3.5 h-3.5 text-discord-blurple" />
              <span>Диагностика P2P и NAT</span>
            </h4>
            <div class="flex justify-between text-discord-text-muted">
              <span>Тип NAT:</span>
              <span class="text-white font-mono">{networkStatus.nat_type}</span>
            </div>
            <div class="flex justify-between text-discord-text-muted">
              <span>Публичный эндпоинт:</span>
              <span class="text-white font-mono">{networkStatus.public_endpoint || 'Определяется (Локальный узел)'}</span>
            </div>
            <div class="flex justify-between text-discord-text-muted">
              <span>UPnP сопоставление:</span>
              <span class="font-bold {networkStatus.is_upnp_mapped ? 'text-discord-green' : 'text-discord-yellow'}">
                {networkStatus.is_upnp_mapped ? 'Активно' : 'Локальный порт 42420'}
              </span>
            </div>
          </div>
        </div>

        <div class="mt-6 flex justify-end">
          <button
            type="button"
            onclick={() => showSettingsModal = false}
            class="px-5 py-2 bg-discord-blurple hover:bg-discord-blurple-hover rounded-xl text-xs font-bold text-white shadow-lg"
          >
            Готово
          </button>
        </div>
      </div>
    </div>
  {/if}

</div>
