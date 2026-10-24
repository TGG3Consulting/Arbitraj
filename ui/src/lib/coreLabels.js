// Файл: ui/src/lib/coreLabels.js
// Что делает: подписи данных ядра по-русски: режим системы, состояние направления на «Живом экране»,
//   шаг сделки, вердикт кандидата. Значения — как их сериализует ядро (src/api/README.md).
// Кто вызывает: components/ControlBar.svelte, components/SpreadCell.svelte, pages/Live.svelte, pages/Deals.svelte.
// От чего зависит: lib/labels.js (showTime).
// Настройки: нет.

import { showTime } from './labels.js';

const MODE = { Stopped: 'Стоп', Running: 'В работе', Paused: 'Пауза' };
/** Режим системы. */
export const modeLabel = (m) => MODE[m] ?? m;

const STEP = {
  Buying: 'покупка',
  Transfer: 'перевод',
  Selling: 'продажа',
  Stuck: 'застрявшая монета',
  WaitingHuman: 'ждёт решения человека',
  Closed: 'закрыта',
  Halted: 'остановлена',
  TakenOver: 'у человека',
};
/** Шаг сделки. */
export const stepLabel = (s) => STEP[s] ?? s;

/**
 * Состояние направления (signal.rs State). Принимает: строку или {вариант: данные}.
 * Возвращает: {text, cls} — подпись и класс цвета.
 */
export function spreadState(s) {
  if (s === 'Below') return { text: 'ниже порога комиссий', cls: 'muted' };
  if (s === 'Holding') return { text: 'держится — ждём фильтр шума', cls: 'warn-text' };
  if (s === 'Candidate') return { text: 'кандидат — в расчёт', cls: 'ok' };
  if (s === 'Anomaly') return { text: 'аномалия — перепроверка статусов', cls: 'err' };
  if (s?.NoPrice !== undefined) return { text: `нет цены: ${s.NoPrice}`, cls: 'muted' };
  if (s?.NotConfigured !== undefined) return { text: `не задано: ${s.NotConfigured}`, cls: 'unset' };
  if (s?.Cooldown) return { text: `пауза до ${showTime(s.Cooldown.until)}`, cls: 'muted' };
  return { text: JSON.stringify(s), cls: '' };
}

/** Направление строкой: «CAKE: bingx → bsc_dex». */
export const dirText = (d) => `${d.asset.ticker}: ${d.buy_on} → ${d.sell_on}`;

/** Ключ направления для списков. */
export const dirKey = (d) => `${d.asset.ticker}|${d.asset.bsc_contract}|${d.buy_on}|${d.sell_on}`;
