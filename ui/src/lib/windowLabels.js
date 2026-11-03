// Файл: ui/src/lib/windowLabels.js
// Что делает: подписи окон (решение 125) — отметки «БЫЛА СДЕЛКА», «плюсовая» / «минусовая», длительность, размер,
//   время из наносекунд с миллисекундами (время получения кадра ботом).
// Кто вызывает: pages/Windows.svelte, components/WindowView.svelte.
// От чего зависит: ничего.
// Настройки: нет.

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
