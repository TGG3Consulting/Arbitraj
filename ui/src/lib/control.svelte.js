// Файл: ui/src/lib/control.svelte.js
// Что делает: режим системы для всего интерфейса (экран «Управление», решение 46): «Стоп» после запуска,
//   «Старт», «Пауза»; паузы площадок, карантины монет, итог дня (решение 173 — и по площадкам), «Сбросить лимит дня»
//   общий и по площадке (решения 153, 173). Данные — GET /api/control, живое
//   сообщение control (src/api/README.md); команды — POST /api/control и снятие паузы/карантина.
// Кто вызывает: App.svelte (start), components/ControlBar.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/flight.js (один запрос в полёте).
// Настройки: нет.

import { get, post, seg } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';

/** Состояние: data — Risk от ядра (null — ещё не получено), error — причина сбоя. */
export const control = $state({ data: null, error: '', busy: false });

/** Перечитывает состояние (один запрос в полёте — lib/flight.js). Ошибка — в control.error. */
const reload = flight(async () => {
  try {
    control.data = await get('/api/control');
    control.error = '';
  } catch (e) {
    control.error = e.message;
  }
});

/** Запускает загрузку и подписку на живой поток (один раз из App.svelte). */
export function start() {
  reload();
  onLive((msg) => {
    if (msg.type === 'control') control.data = msg.data;
    else if (lagFor(msg, 'core')) reload();
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

/** Решение 230: снять паузу кошелька DEX от сверки. Принимает: id кошелька (w1…). */
export const resumeWallet = (id) => command(`/api/control/wallet/${seg(id)}/resume`);

/** Снять карантин монеты. Принимает: id — адрес контракта (решение 118) или тикер монеты без адреса. */
export const releaseCoin = (t) => command(`/api/control/quarantine/${seg(t)}/release`);

/** «Сбросить лимит дня» общий (решения 153, 173): вся система и все площадки, паузы площадок по убытку дня снимаются;
 *  режим не меняется; подтверждение — в ControlBar.svelte. */
export const resetDailyLimit = () => command('/api/control/daily-limit/reset');

/** «Сбросить лимит дня» одной площадки (решение 173): её пауза по убытку дня снимается; подтверждение — в
 *  DayVenues.svelte. Принимает: id площадки. */
export const resetVenueLimit = (id) => command(`/api/control/venue/${seg(id)}/daily-limit/reset`);
