// Файл: ui/src/lib/coinNames.svelte.js
// Что делает: решение 211 (монета = адрес контракта BSC) — справочник имён монет по адресу для всех экранов:
//   GET /api/coins/names → адрес → {ticker (показной), venues: {биржа: тикер этой биржи}}. Отсюда имя монеты
//   «NMT / NETMIND» (уникальные тикеры площадок через « / », первым показной), тикер своей площадки (делистинг)
//   и все тикеры монеты (фильтр). Перечитывается по живому сообщению coin_check (справочник пересобран) и «lagged».
//   Нет справочника (503, ядро не запущено) — имя = тикер монеты из её данных, как раньше.
// Кто вызывает: lib/coreLabels.js, lib/delist.svelte.js, lib/coinFilter.js, pages/Live.svelte, LiveCoins и др.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: нет.

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';

/** Адрес (нижний регистр) → {ticker, venues}. */
export const names = $state({ map: {} });

let started = false;

/** Перечитывает справочник имён. Ошибка — имена остаются прежними (показ по тикеру монеты). */
async function load() {
  try {
    const list = await get('/api/coins/names');
    names.map = Object.fromEntries(list.map((c) => [c.address.toLowerCase(), c]));
  } catch {
    // Нет справочника — показ по тикеру монеты (Asset.ticker).
  }
}

/** Запускает загрузку и подписку один раз за страницу. Вызывается при первом обращении. */
function ensure() {
  if (started) return;
  started = true;
  load();
  onLive((m) => {
    if (m.type === 'coin_check' || lagFor(m, 'coin_check')) load();
  });
}

/** Ключ монеты: адрес в нижнем регистре; у монеты без адреса — тикер. Принимает: Asset. */
export const assetId = (a) => (a.bsc_contract ? a.bsc_contract.toLowerCase() : a.ticker);

/** Запись справочника по монете или undefined. Принимает: Asset. */
const entry = (a) => {
  ensure();
  return a.bsc_contract ? names.map[a.bsc_contract.toLowerCase()] : undefined;
};

/** Все тикеры монеты: показной первым, затем уникальные тикеры площадок. Принимает: Asset. Возвращает: список. */
export const tickersOf = (a) => {
  const e = entry(a);
  return [...new Set([a.ticker, ...(e ? [e.ticker, ...Object.values(e.venues)] : [])])];
};

/** Имя монеты «NMT / NETMIND». Принимает: Asset. Возвращает: строку. */
export const coinName = (a) => tickersOf(a).join(' / ');

/** Тикер монеты на площадке (для делистинга и запросов показа); нет записи — тикер монеты. Принимает: Asset, id. */
export const tickerOn = (a, venue) => entry(a)?.venues?.[venue] ?? a.ticker;

/** Подходит ли монета под строку поиска: любой тикер площадок или адрес. Принимает: Asset, строку. */
export const assetMatches = (a, q) => {
  const s = q.trim().toLowerCase();
  if (!s) return true;
  return tickersOf(a).some((t) => t.toLowerCase().includes(s)) || (a.bsc_contract ?? '').toLowerCase().includes(s);
};
