// Файл: ui/src/lib/coinNames.svelte.js
// Что делает: решение 211 (монета = адрес контракта BSC) — справочник имён монет по адресу для всех экранов:
//   GET /api/coins/names → адрес → {ticker (показной), venues: {биржа: тикер этой биржи}}. Отсюда имя монеты
//   «NMT / NETMIND» (уникальные тикеры площадок через « / », первым показной), тикер своей площадки (делистинг)
//   и все тикеры монеты (фильтр). Перечитывается по живому сообщению coins (ядро пересобрало справочник монет —
//   core/rebuild.rs; GET /api/coins/names строится из того же справочника, api/coins.rs names) и по «lagged» ядра.
//   Шторм 10.11.2026: раньше перечитывался на КАЖДОЕ сообщение coin_check (итог проверки налогов одной монеты —
//   тысячи за цикл проверки, справочник имён от них не меняется) без single-flight — браузер исчерпал лимит
//   запросов (893 × net::ERR_INSUFFICIENT_RESOURCES). Теперь — по coins и один запрос в полёте (lib/flight.js).
//   Нет справочника (503, ядро не запущено) — имя = тикер монеты из её данных, как раньше.
// Кто вызывает: lib/coreLabels.js, lib/delist.svelte.js, lib/coinFilter.js, pages/Live.svelte, LiveCoins и др.
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/flight.js.
// Настройки: нет.

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';

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

/** Перечитывание справочника: один запрос в полёте, вызов во время запроса — один повтор после него. */
const reload = flight(load);

/**
 * Запускает загрузку и подписку один раз за страницу. Вызывается при первом обращении через функции ниже; экран,
 * который читает names.map напрямую (ManualBlacklist, SettingCard), вызывает его сам (ревью М1).
 */
export function ensure() {
  if (started) return;
  started = true;
  reload();
  onLive((m) => {
    if (m.type === 'coins' || lagFor(m, 'core')) reload();
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

/** Адрес контракта BSC: 0x и 40 шестнадцатеричных знаков (как settings/scope.rs is_address_id). */
export const ADDR = /^0x[0-9a-fA-F]{40}$/;

/** Адреса монет справочника с этим тикером (показной или любой площадки), без повторов. Принимает: тикер. */
export function addressesOf(t) {
  ensure();
  const u = t.toUpperCase();
  const list = Object.values(names.map)
    .filter((c) => c.ticker.toUpperCase() === u || Object.values(c.venues).some((x) => x.toUpperCase() === u))
    .map((c) => c.address.toLowerCase());
  return [...new Set(list)];
}

/**
 * Область «монета» по вводу человека (решение 211: монета = адрес): адрес — как есть, в нижнем регистре; тикер — его
 * единственный адрес по справочнику имён. Принимает: строку. Возвращает: id области (адрес). Ошибки: у тикера
 * несколько адресов или ни одного — текст с просьбой ввести адрес.
 */
export function coinIdOf(v) {
  const s = v.trim();
  if (ADDR.test(s)) return s.toLowerCase();
  const a = addressesOf(s);
  if (a.length === 1) return a[0];
  if (a.length > 1) throw new Error(`У тикера ${s} несколько адресов (${a.join(', ')}) — введите адрес монеты.`);
  throw new Error(`Монеты с адресом под тикером ${s} в справочнике нет — введите адрес монеты (0x…).`);
}
