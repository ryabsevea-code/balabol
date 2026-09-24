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
    Terminal,
    FileText,
    ArrowRight,
    Send,
    MessageSquare,
    Key,
    ShieldCheck
  } from 'lucide-svelte';

  let status = { type: 'Disconnected' };
  let peers = [];
  let logs = [];
  let chatMessages = [];
  let logFilePath = '';
  let pollInterval = null;

  // Modals state
  let showConnectModal = false;

  // User info
  let nickname = localStorage.getItem('balabol_nickname') || 'Узел-' + Math.floor(1000 + Math.random() * 9000);
  let roomCodeInput = '';

  // Chat input
  let chatInput = '';
  let chatContainer;
  let logContainer;

  let copiedItem = '';
  let isSubmitting = false;

  onMount(async () => {
    try {
      logFilePath = await invoke('get_log_file_path');
    } catch (e) {
      console.error('Failed to get log path:', e);
    }

    await pollState();
    pollInterval = setInterval(pollState, 350);
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
  });

  async function pollState() {
    try {
      status = await invoke('get_status');
      peers = await invoke('get_peers');
      
      if (status.type === 'Connected') {
        const newChat = await invoke('get_chat_messages');
        if (newChat.length !== chatMessages.length) {
          chatMessages = newChat;
          scrollToChatBottom();
        }
      }

      const newLogs = await invoke('get_logs');
      if (newLogs.length !== logs.length) {
        logs = newLogs;
        scrollToLogBottom();
      }
    } catch (e) {
      console.error('Poll error:', e);
    }
  }

  function scrollToChatBottom() {
    setTimeout(() => {
      if (chatContainer) {
        chatContainer.scrollTop = chatContainer.scrollHeight;
      }
    }, 40);
  }

  function scrollToLogBottom() {
    setTimeout(() => {
      if (logContainer) {
        logContainer.scrollTop = logContainer.scrollHeight;
      }
    }, 40);
  }

  function saveNickname(name) {
    nickname = name;
    localStorage.setItem('balabol_nickname', name);
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

  // --- Actions: Chat ---
  async function handleSendMessage() {
    const text = chatInput.trim();
    if (!text) return;
    chatInput = '';
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
          <span class="text-[10px] uppercase font-semibold tracking-wider bg-blue-500/10 text-blue-400 border border-blue-500/20 px-1.5 py-0.5 rounded">Прямая связь</span>
        </div>
        <div class="text-xs text-[#8a8e98] flex items-center gap-1.5">
          <span>Ваш никнейм:</span>
          <span class="text-white font-medium">{nickname}</span>
        </div>
      </div>
    </div>

    <!-- Right Controls / Status -->
    <div class="flex items-center gap-3">
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
        <div class="flex items-center gap-2 px-3 py-1 rounded-full bg-emerald-500/10 text-emerald-400 border border-emerald-500/20 text-xs font-medium">
          <div class="w-2 h-2 rounded-full bg-emerald-500"></div>
          P2P: {status.peer_name}
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
    <main class="flex-1 flex flex-col p-6 overflow-y-auto bg-[#14151a]">
      {#if status.type === 'Disconnected'}
        <!-- Disconnected Zero State -->
        <div class="max-w-xl mx-auto w-full my-auto flex flex-col items-center text-center">
          <div class="w-16 h-16 rounded-2xl bg-[#1c1e24] border border-[#2a2d36] flex items-center justify-center mb-5 text-blue-400 shadow-xl">
            <Radio class="w-8 h-8" />
          </div>
          <h2 class="text-2xl font-bold text-white mb-2">Прямое P2P подключение</h2>
          <p class="text-sm text-[#8a8e98] mb-8 max-w-md leading-relaxed">
            Соединение по 6-значному коду через интернет. Работает через VPN, сотовую сеть и домашние роутеры.
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
                Сгенерирует 6-значный код комнаты и будет ожидать подключения собеседника.
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
                  Войти
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
              Ожидание подключения собеседника
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
        <!-- Connected State with Real-Time P2P Chat -->
        <div class="max-w-4xl mx-auto w-full h-full flex flex-col gap-4">
          <!-- Top Connected Info Bar -->
          <div class="p-4 rounded-xl bg-gradient-to-r from-emerald-950/40 to-[#181a20] border border-emerald-500/30 flex items-center justify-between shrink-0 shadow-lg">
            <div class="flex items-center gap-3">
              <div class="relative">
                <div class="w-10 h-10 rounded-full bg-gradient-to-tr from-emerald-600 to-teal-500 flex items-center justify-center font-bold text-white text-sm ring-2 ring-emerald-500/40">
                  {status.peer_name.slice(0, 2).toUpperCase()}
                </div>
                <div class="absolute -bottom-0.5 -right-0.5 w-3 h-3 rounded-full bg-emerald-500 border-2 border-[#181a20]"></div>
              </div>
              <div>
                <div class="flex items-center gap-2">
                  <span class="font-bold text-white text-sm">{status.peer_name}</span>
                  <span class="text-[10px] bg-emerald-500/20 text-emerald-300 border border-emerald-500/30 px-1.5 py-0.2 rounded font-semibold flex items-center gap-1">
                    <ShieldCheck class="w-3 h-3" /> P2P Прямое
                  </span>
                </div>
                <span class="text-xs text-[#8a8e98] font-mono">{status.peer_addr}</span>
              </div>
            </div>

            <div class="flex items-center gap-3">
              <div class="bg-[#121317] px-3 py-1.5 rounded-lg border border-emerald-500/30 flex items-center gap-2">
                <span class="text-xs text-[#8a8e98]">RTT Пинг:</span>
                <span class="text-sm font-mono font-bold text-emerald-400">{status.ping_ms} мс</span>
              </div>
              <button
                on:click={handleDisconnect}
                class="px-3.5 py-1.5 bg-red-600/80 hover:bg-red-600 text-white rounded-lg text-xs font-semibold transition"
              >
                Отключиться
              </button>
            </div>
          </div>

          <!-- Main P2P Chat Box -->
          <div class="flex-1 flex flex-col bg-[#16181f] border border-[#262832] rounded-2xl overflow-hidden shadow-2xl">
            <!-- Chat Header -->
            <div class="h-11 bg-[#1a1c24] border-b border-[#262832] px-4 flex items-center justify-between text-xs font-semibold text-[#8a8e98]">
              <div class="flex items-center gap-2 text-white">
                <MessageSquare class="w-4 h-4 text-blue-400" />
                <span>Прямой текстовый P2P канал передачи</span>
              </div>
              <span class="text-[11px] text-[#606470]">Пакеты идут напрямую между ПК без сервера</span>
            </div>

            <!-- Messages Stream -->
            <div
              bind:this={chatContainer}
              class="flex-1 p-4 overflow-y-auto space-y-3 font-sans select-text"
            >
              {#if chatMessages.length === 0}
                <div class="h-full flex flex-col items-center justify-center text-center text-xs text-[#606470] space-y-2">
                  <MessageSquare class="w-8 h-8 text-[#303340]" />
                  <span>Канал P2P открыт! Напишите тестовое сообщение.</span>
                </div>
              {:else}
                {#each chatMessages as msg (msg.id + '-' + (msg.is_me ? 'me' : 'peer') + '-' + msg.timestamp_ms)}
                  <div class="flex flex-col {msg.is_me ? 'items-end' : 'items-start'}">
                    <div class="flex items-center gap-1.5 mb-1 px-1 text-[11px] text-[#707482]">
                      <span class="font-semibold text-white">{msg.sender}</span>
                      <span>•</span>
                      <span>{new Date(msg.timestamp_ms).toLocaleTimeString()}</span>
                    </div>
                    <div
                      class="max-w-md px-3.5 py-2 rounded-2xl text-xs leading-relaxed shadow-md break-words {msg.is_me ? 'bg-blue-600 text-white rounded-tr-none' : 'bg-[#222530] text-[#e0e2e8] border border-[#2d303e] rounded-tl-none'}"
                    >
                      {msg.text}
                    </div>
                  </div>
                {/each}
              {/if}
            </div>

            <!-- Chat Input Area -->
            <div class="p-3 bg-[#13141a] border-t border-[#262832] flex items-center gap-2">
              <input
                type="text"
                bind:value={chatInput}
                on:keydown={handleChatKeyDown}
                placeholder="Введите сообщение и нажмите Enter..."
                class="flex-1 bg-[#1a1c24] border border-[#2b2d38] rounded-xl px-4 py-2.5 text-xs text-white placeholder-[#555866] focus:outline-none focus:border-blue-500 transition"
              />
              <button
                on:click={handleSendMessage}
                disabled={!chatInput.trim()}
                class="px-4 py-2.5 bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-white rounded-xl text-xs font-semibold flex items-center gap-1.5 transition shadow-md shadow-blue-600/20"
              >
                <span>Отправить</span>
                <Send class="w-3.5 h-3.5" />
              </button>
            </div>
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

    <!-- Bottom Log Terminal Console (TeamSpeak Event Log) -->
    <aside class="h-40 bg-[#0e0f12] border-t border-[#20222a] flex flex-col shrink-0">
      <div class="h-8 bg-[#14151b] px-4 flex items-center justify-between border-b border-[#20222a] text-xs">
        <div class="flex items-center gap-2 text-[#8a8e98] font-semibold">
          <Terminal class="w-3.5 h-3.5 text-blue-400" />
          <span>Журнал сетевых событий (TeamSpeak Event Console)</span>
        </div>
        <div class="flex items-center gap-3 text-[11px] text-[#606470]">
          <span class="flex items-center gap-1 font-mono">
            <FileText class="w-3 h-3 text-[#707482]" />
            {logFilePath || 'balabol.log'}
          </span>
          <button
            on:click={() => logs = []}
            class="hover:text-white transition"
          >
            Очистить экран
          </button>
        </div>
      </div>

      <div
        bind:this={logContainer}
        class="flex-1 p-3 overflow-y-auto font-mono text-[11px] leading-5 space-y-0.5 select-text"
      >
        {#if logs.length === 0}
          <div class="text-[#454854] italic">Журнал пуст. Ожидание событий...</div>
        {:else}
          {#each logs as log}
            <div class={log.includes('ERROR') ? 'text-red-400' : log.includes('WARN') ? 'text-amber-300' : log.includes('УСПЕХ') || log.includes('УСТАНОВЛЕНО НАПРЯМУЮ') ? 'text-emerald-400 font-bold' : log.includes('[ЧАТ]') ? 'text-blue-300 font-semibold' : 'text-[#8a8e98]'}>
              {log}
            </div>
          {/each}
        {/if}
      </div>
    </aside>
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
</div>
