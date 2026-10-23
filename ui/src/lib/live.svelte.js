// Файл: ui/src/lib/live.svelte.js
// Что делает: одно WebSocket-подключение к /api/ws на всё приложение и рассылка сообщений экранам.
//   Сообщения бэкенда (src/api/ws.rs): {"type":"settings","data":…}, {"type":"journal","data":…},
//   {"type":"lagged","skipped":N} — интерфейс отстал, данные нужно перечитать по REST.
//   После обрыва переподключается по событию, без таймеров: вкладка снова видна, окно в фокусе,
//   сеть вернулась, или кнопка «Переподключиться» в плашке.
// Кто вызывает: App.svelte (connect), экраны и хранилища (onLive), components/LiveBanner.svelte (live).
// От чего зависит: WebSocket браузера.
// Настройки: нет.

/** Состояние живой связи: connected — подключено; reason — причина обрыва. */
export const live = $state({ connected: false, reason: '' });

const listeners = new Set();
let socket = null;

/**
 * Что делает: подписка на сообщения живого потока.
 * Принимает: функцию (сообщение) => void. Возвращает: функцию отписки.
 */
export function onLive(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Что делает: открывает подключение, если его нет. Ошибки: показываются в live.reason. */
export function connect() {
  if (socket && socket.readyState <= WebSocket.OPEN) return;
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${proto}//${location.host}/api/ws`);
  socket = ws;
  ws.onopen = () => {
    live.connected = true;
    live.reason = '';
    // После переподключения могли пропустить события — просим всех перечитать данные.
    emit({ type: 'lagged', skipped: null });
  };
  ws.onmessage = (ev) => {
    try {
      emit(JSON.parse(ev.data));
    } catch (e) {
      live.reason = `Непонятное сообщение сервера: ${e.message}`;
    }
  };
  ws.onclose = (ev) => {
    if (socket !== ws) return;
    live.connected = false;
    live.reason = ev.reason || `соединение закрыто (код ${ev.code})`;
    socket = null;
  };
}

/** Рассылает сообщение всем подписчикам. Принимает: сообщение. */
function emit(msg) {
  for (const fn of listeners) fn(msg);
}

/** Что делает: включает переподключение по событиям браузера. Вызывается один раз из App.svelte. */
export function reconnectOnEvents() {
  const again = () => {
    if (!live.connected && document.visibilityState === 'visible') connect();
  };
  document.addEventListener('visibilitychange', again);
  window.addEventListener('focus', again);
  window.addEventListener('online', again);
}
