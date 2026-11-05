// Файл: ui/src/lib/delist.svelte.js
// Что делает: делистинги бирж для всего интерфейса (решение 175) — по каждой бирже последний ответ (пары в делистинге
//   или «неизвестно» с причиной), время ответа и ошибка запроса; поиск записи монеты по (площадка, тикер) для отметок
//   «делистинг на X: дата» на «Монетах» и «Живом экране». Данные — GET /api/delistings; перечитываются раз в минуту и
//   при отставании живого потока ядра (обновление у ядра — delist.refresh_s, обычно раз в час).
// Кто вызывает: pages/Venues.svelte (components/Delistings.svelte), components/CoinRow.svelte, pages/Live.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: нет (delist.refresh_s, delist.block_hours — экран «Настройки» / «Площадки»).

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';

/** Период перечитывания экрана, мс — частота показа, не торговый параметр. */
const RELOAD_MS = 60000;

/** rows — ответ GET /api/delistings ([{venue, got, at, error}]); error — сбой запроса. */
export const delist = $state({ rows: [], error: '' });

let started = false;

/** Перечитывает делистинги. Ошибка — в delist.error. */
async function reload() {
  try {
    delist.rows = await get('/api/delistings');
    delist.error = '';
  } catch (e) {
    delist.error = e.message;
  }
}

/** Запускает загрузку один раз (повторные вызовы ничего не делают). */
export function startDelist() {
  if (started) return;
  started = true;
  reload();
  setInterval(reload, RELOAD_MS);
  onLive((msg) => {
    if (lagFor(msg, 'core')) reload();
  });
}

/** Состояние одной площадки. Принимает: id. Возвращает: строку ответа или undefined. */
export const delistOf = (venue) => delist.rows.find((r) => r.venue === venue);

/** Запись делистинга монеты на площадке. Принимает: id площадки, тикер. Возвращает: DelistItem или undefined. */
export const delistItem = (venue, ticker) => delistOf(venue)?.got?.items?.find((i) => i.ticker === ticker);

/** Время для глаз: «05.11 00:00 UTC». Принимает: ISO-строку. */
export const delistTime = (t) => {
  const d = new Date(t);
  const p = (n) => String(n).padStart(2, '0');
  return `${p(d.getUTCDate())}.${p(d.getUTCMonth() + 1)} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())} UTC`;
};

/** Отметка монеты: «делистинг на kucoin: 05.11 00:00 UTC (вывод до 07.12 08:00 UTC)». Принимает: площадку, запись. */
export const delistLabel = (venue, i) => {
  const t = i.trading_end ? delistTime(i.trading_end) : 'время не сообщено';
  const w = i.withdraw_end ? ` (вывод до ${delistTime(i.withdraw_end)})` : '';
  return `делистинг на ${venue}: ${t}${w}`;
};

/** Отметки направления (обе стороны). Принимает: Direction. Возвращает: список строк. */
export const delistMarks = (dir) =>
  [dir.buy_on, dir.sell_on]
    .map((v) => [v, delistItem(v, dir.asset.ticker)])
    .filter(([, i]) => i)
    .map(([v, i]) => delistLabel(v, i));
