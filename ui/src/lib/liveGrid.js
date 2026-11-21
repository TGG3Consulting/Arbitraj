// Файл: ui/src/lib/liveGrid.js
// Что делает: раскладка «Живого экрана» по монетам (решение 55, вид «Б» — решение 106): из списка SpreadView
//   (по направлениям) собирает строки «одна монета» с лучшим направлением и списком всех её направлений.
//   Ширина таблицы не зависит от числа площадок — новая биржа добавляет направления в раскрытие строки, а не колонки.
// Кто вызывает: pages/Live.svelte, components/LiveCoins.svelte.
// От чего зависит: lib/coinNames.svelte.js (ключ монеты по адресу, решение 211); формат — src/api/README.md.
// Настройки: нет.

/** Состояния, при которых спред выше порога комиссий (signal.rs State). */
const ACTIVE = ['Holding', 'Candidate', 'Anomaly'];

import { assetId } from './coinNames.svelte.js';

/**
 * Ключ монеты (решение 211): адрес контракта; у монеты без адреса — тикер.
 * Принимает: Asset {ticker, bsc_contract}. Возвращает: строку.
 */
export const coinKey = (a) => assetId(a);

/**
 * Ключ направления внутри строки монеты: «купить на A → продать на B».
 * Принимает: id площадки покупки и продажи. Возвращает: строку.
 */
export const legKey = (buy, sell) => `${buy}>${sell}`;

/** Спред направления числом; нет спреда — null. */
const pct = (v) => (v?.spread_pct == null ? null : Number(v.spread_pct));

/**
 * Строки монет. Принимает: SpreadView[].
 * Возвращает: [{key, asset, legs: {legKey: SpreadView}, best: число | null, bestKey, active: bool, activeCount, at}] —
 *   best — наибольший спред из направлений монеты; bestKey — лучшее направление (решение 106: среди направлений выше
 *   порога — с наибольшим спредом, нет таких — просто с наибольшим спредом); active — хоть одно направление выше
 *   порога; activeCount — сколько их; at — самое свежее обновление.
 */
export function coinRows(views) {
  const rows = new Map();
  for (const v of views) {
    const d = v.direction;
    const k = coinKey(d.asset);
    if (!rows.has(k)) rows.set(k, { key: k, asset: d.asset, legs: {}, best: null, bestKey: null, active: false, activeCount: 0, at: v.at });
    const r = rows.get(k);
    r.legs[legKey(d.buy_on, d.sell_on)] = v;
    const p = pct(v);
    if (p != null && (r.best == null || p > r.best)) r.best = p;
    if (ACTIVE.includes(v.state)) {
      r.active = true;
      r.activeCount += 1;
    }
    if (v.at > r.at) r.at = v.at;
  }
  for (const r of rows.values()) r.bestKey = bestLeg(r.legs);
  return [...rows.values()];
}

/**
 * Лучшее направление монеты (решение 106): среди выше порога — с наибольшим спредом; нет таких — с наибольшим спредом;
 * спредов нет — первое по ключу. Принимает: {legKey: SpreadView}. Возвращает: legKey или null.
 */
function bestLeg(legs) {
  const keys = Object.keys(legs).sort();
  const score = (k) => pct(legs[k]) ?? -Infinity;
  const pick = (list) => list.reduce((b, k) => (b == null || score(k) > score(b) ? k : b), null);
  const hot = keys.filter((k) => ACTIVE.includes(legs[k].state));
  return pick(hot.length ? hot : keys);
}

/**
 * Направления строки монеты по убыванию спреда (для раскрытия строки и окна).
 * Принимает: строку coinRows. Возвращает: [{buy, sell, key}].
 */
export function rowLegs(row) {
  return Object.entries(row.legs)
    .map(([key, v]) => ({ buy: v.direction.buy_on, sell: v.direction.sell_on, key }))
    .sort((x, y) => (pct(row.legs[y.key]) ?? -Infinity) - (pct(row.legs[x.key]) ?? -Infinity));
}

/**
 * Сортировка по умолчанию: наибольший спред из двух (и более) направлений, монеты без спреда — в конце,
 * при равенстве — по тикеру. Принимает: строки coinRows. Возвращает: новый массив.
 */
export const byBestSpread = (rows) =>
  [...rows].sort((x, y) => {
    if (x.best == null && y.best == null) return x.asset.ticker.localeCompare(y.asset.ticker);
    if (x.best == null) return 1;
    if (y.best == null) return -1;
    return y.best - x.best || x.asset.ticker.localeCompare(y.asset.ticker);
  });
