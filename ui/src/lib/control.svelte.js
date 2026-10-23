// Файл: ui/src/lib/control.svelte.js
// Что делает: режим системы для всего интерфейса (экран «Управление», решение 46): «Стоп» после запуска,
//   «Старт», «Пауза»; паузы площадок, карантины монет, итог дня. Данные — GET /api/control, живое
//   сообщение control (src/api/README.md); команды — POST /api/control и снятие паузы/карантина.
// Кто вызывает: App.svelte (start), components/ControlBar.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: нет.

import { get, post, seg } from './api.js';
import { onLive } from './live.svelte.js';

/** Состояние: data — Risk от ядра (null — ещё не получено), error — причина сбоя. */
export const control = $state({ data: null, error: '', busy: false });

/** Перечитывает состояние. Ошибка — в control.error. */
async function reload() {
  try {
    control.data = await get('/api/control');
    control.error = '';
  } catch (e) {
    control.error = e.message;
  }
}

/** Запускает загрузку и подписку на живой поток (один раз из App.svelte). */
export function start() {
  reload();
  onLive((msg) => {
    if (msg.type === 'control') control.data = msg.data;
    else if (msg.type === 'lagged') reload();
  });
}

/** Выполняет команду и берёт ответ (Risk). Принимает: путь, тело. */
async function command(path, body) {
  control.busy = true;
  try {
    control.data = await post(path, body);
    control.error = '';
  } catch (e) {
    control.error = e.message;
  }
  control.busy = false;
}

/** «Старт» / «Стоп» / «Пауза». Принимает: 'Running' | 'Stopped' | 'Paused'. */
export const setMode = (mode) => command('/api/control', { mode });

/** Снять паузу площадки. Принимает: id площадки. */
export const resumeVenue = (id) => command(`/api/control/venue/${seg(id)}/resume`);

/** Снять карантин монеты. Принимает: тикер. */
export const releaseCoin = (t) => command(`/api/control/quarantine/${seg(t)}/release`);
