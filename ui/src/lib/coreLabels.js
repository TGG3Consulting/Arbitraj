// Файл: ui/src/lib/coreLabels.js
// Что делает: подписи данных ядра по-русски: режим системы, состояние направления на «Живом экране»,
//   шаг сделки, вердикт кандидата. Значения — как их сериализует ядро (src/api/README.md).
//   Короткое состояние для компактной строки и причина отдельно — для всплывающего окна (Issue #33).
// Кто вызывает: components/ControlBar.svelte, components/SpreadCell.svelte, components/LiveCoinModal.svelte,
//   pages/Live.svelte, pages/Deals.svelte.
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
  // Решение 61: налоги монеты ещё не проверены — строка видна, торговли нет (текст ядра уже с причиной).
  if (s?.TaxesNotChecked !== undefined) return { text: s.TaxesNotChecked, cls: 'warn-text' };
  return { text: JSON.stringify(s), cls: '' };
}

/**
 * Короткое состояние направления для строки «Живого экрана» (одно-два слова, без причины).
 * Принимает: State (строка или {вариант: данные}). Возвращает: {text, cls, icon}.
 */
export function spreadShort(s) {
  if (s === 'Below') return { text: 'ниже порога', cls: 'muted', icon: '○' };
  if (s === 'Holding') return { text: 'держится', cls: 'warn-text', icon: '◐' };
  if (s === 'Candidate') return { text: 'кандидат', cls: 'ok', icon: '●' };
  if (s === 'Anomaly') return { text: 'аномалия', cls: 'err', icon: '!' };
  if (s?.NoPrice !== undefined) return { text: 'нет цены', cls: 'muted', icon: '—' };
  if (s?.NotConfigured !== undefined) return { text: 'не задано', cls: 'unset', icon: '?' };
  if (s?.Cooldown) return { text: 'пауза', cls: 'muted', icon: '‖' };
  if (s?.TaxesNotChecked !== undefined) return { text: 'налоги не проверены', cls: 'warn-text', icon: '…' };
  return { text: JSON.stringify(s), cls: '', icon: '' };
}

/**
 * Причина состояния целиком (для всплывающего окна): текст ядра у NoPrice / NotConfigured,
 * «до <время>» у Cooldown. Принимает: State. Возвращает: строку или null — причины нет.
 */
export function spreadReason(s) {
  if (s?.NoPrice !== undefined) return s.NoPrice;
  if (s?.NotConfigured !== undefined) return s.NotConfigured;
  if (s?.Cooldown) return `пауза до ${showTime(s.Cooldown.until)}`;
  if (s?.TaxesNotChecked !== undefined) return s.TaxesNotChecked;
  return null;
}

/** Направление строкой: «CAKE: bingx → bsc_dex». */
export const dirText = (d) => `${d.asset.ticker}: ${d.buy_on} → ${d.sell_on}`;

/** Ключ направления для списков. */
export const dirKey = (d) => `${d.asset.ticker}|${d.asset.bsc_contract}|${d.buy_on}|${d.sell_on}`;
