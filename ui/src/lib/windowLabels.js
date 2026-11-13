// Файл: ui/src/lib/windowLabels.js
// Что делает: подписи окон (решение 125) — отметки «БЫЛА СДЕЛКА», «плюсовая» / «минусовая», длительность, размер,
//   время из наносекунд с миллисекундами (время получения кадра ботом); отказы предпроверки сделки окна (решение 266);
//   сводки теневых расчётов «пока монета занята сделкой» (решение 267).
// Кто вызывает: pages/Windows.svelte, components/WindowView.svelte.
// От чего зависит: lib/fmt.js (fmtUsd).
// Настройки: нет.

import { fmtUsd } from './fmt.js';

/** Итог сделок окна → подпись и класс. Принимает: outcome сводки. Возвращает: {text, cls} или null. */
export function outcomeMark(o) {
  if (o === 'plus') return { text: 'плюсовая', cls: 'ok' };
  if (o === 'minus') return { text: 'минусовая', cls: 'err' };
  if (o === 'zero') return { text: 'в ноль', cls: 'muted' };
  return null;
}

/** Длительность окна, секунд (у открытого — до now). Принимает: сводку, текущее время мс. */
export const winSecs = (w, now) =>
  (((w.ended_at ? Date.parse(w.ended_at) : now) - Date.parse(w.started_at)) / 1000).toFixed(1);

/** Размер в КБ/МБ. Принимает: байты. */
export function size(b) {
  if (b >= 1048576) return `${(b / 1048576).toFixed(2)} МБ`;
  return `${(b / 1024).toFixed(1)} КБ`;
}

/** Время с миллисекундами. Принимает: Unix мс. */
export function msTime(ms) {
  if (ms == null) return '—';
  const d = new Date(ms);
  return `${d.toLocaleTimeString('ru-RU')}.${String(d.getMilliseconds()).padStart(3, '0')}`;
}

/** Время получения кадра (нс) с микросекундами. Принимает: Unix нс. */
export function nsTime(ns) {
  const ms = Math.floor(ns / 1e6);
  const us = String(Math.floor((ns % 1e6) / 1e3)).padStart(3, '0');
  return `${msTime(ms)}${us}`;
}

/** Направления окна текстом. Принимает: сводку. */
export const winDirs = (w) => (w.directions ?? []).map((d) => `${d.buy_on} → ${d.sell_on}`).join(', ');

/**
 * Отказы предпроверки сделки окна (решение 266, сводка окна rejects) — строки для показа: направление, причина без
 * хвоста «(время: …)», сколько раз, метка «баланс»; полный текст последнего отказа — в подсказке.
 * Принимает: сводку. Возвращает: [{key, dir, text, count, balance, full}].
 */
export const winRejects = (w) =>
  (w.rejects ?? []).map((r, i) => ({
    key: `${r.direction.buy_on}|${r.direction.sell_on}|${i}`,
    dir: `${r.direction.buy_on} → ${r.direction.sell_on}`,
    text: r.reason.split(' (время: ')[0],
    count: r.count,
    balance: r.balance,
    full: r.reason,
  }));

/** Во сколько раз вложение теневого максимума больше / меньше вложения сделки (факт; до покупки — план). */
function busyRatio(b) {
  const v = Number(b.best?.invested);
  const fact = Number(b.deal?.invested);
  const base = fact > 0 ? fact : Number(b.deal?.plan_invested);
  if (!(v > 0) || !(base > 0)) return '';
  if (v > base) return ` (в ${(v / base).toFixed(2)} раз больше сделки)`;
  if (v < base) return ` (в ${(base / v).toFixed(2)} раз меньше сделки)`;
  return ' (как у сделки)';
}

/**
 * Сводки теневых расчётов окна «пока монета занята сделкой» (решение 267, сводка окна busy; замер включается
 * window.busy_calc) — строки для показа: направление, сделка, сколько расчётов и сколько прошло пороги, максимум
 * прибыли и вложение при нём со сравнением со сделкой, наибольшее прибыльное вложение; подсказка — бюджет расчёта,
 * время и последний отказ. Принимает: сводку. Возвращает: [{key, dir, text, full}].
 */
export const winBusy = (w) =>
  (w.busy ?? []).map((b, i) => {
    const dir = `${b.direction.buy_on} → ${b.direction.sell_on}`;
    const deal = b.deal
      ? `сделкой ${b.deal.id} (вложено ${fmtUsd(b.deal.invested)}, план ${fmtUsd(b.deal.plan_profit_usd)})`
      : '(сделки в учёте не видно)';
    const dropped = b.dropped ? `, отброшено ${b.dropped}` : '';
    const top = b.best
      ? `максимум прибыли ${fmtUsd(b.best.profit_usd)} при вложении ${fmtUsd(b.best.invested)}${busyRatio(b)}; наибольшее прибыльное вложение ${fmtUsd(b.max_invested)}`
      : 'прибыльных расчётов не было';
    const time = (t) => new Date(t).toLocaleTimeString('ru-RU');
    const full = [
      `с ${time(b.first_at)} по ${time(b.last_at)}`,
      b.best ? `максимум в ${time(b.best.at)}` : null,
      b.best?.budget != null ? `бюджет расчёта ${fmtUsd(b.best.budget)}` : null,
      b.deal?.plan_invested != null ? `вложение плана сделки ${fmtUsd(b.deal.plan_invested)}` : null,
      b.last_reject ? `последний отказ: ${b.last_reject}` : null,
    ]
      .filter(Boolean)
      .join('; ');
    return {
      key: `${b.direction.buy_on}|${b.direction.sell_on}|${i}`,
      dir,
      text: `занято ${deal}: теневых расчётов ${b.calcs}, прошло пороги ${b.accepted}${dropped}; ${top}`,
      full,
    };
  });
