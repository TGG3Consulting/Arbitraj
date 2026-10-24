// Файл: ui/src/lib/fmt.js
// Что делает: короткий показ чисел, адресов, времени и причин в компактных строках (Issue #33):
//   проценты — 3 знака после точки, цены — до 6 значащих цифр, адрес — «0x1234…abcd», время — только часы.
//   Полные значения показываются как есть во всплывающих окнах (они приходят строками Decimal из ядра).
//   Это правила показа, а не параметры торговли: на расчёты не влияют.
// Кто вызывает: components/SpreadCell.svelte, LiveCoins.svelte, LiveCoinModal.svelte, CoinTable.svelte.
// От чего зависит: ничего.
// Настройки: нет.

/** Знаков после точки у процентов в компактной строке (Issue #33: «2–3 знака»). */
const PCT_DIGITS = 3;
/** Значащих цифр у цены в компактном виде (Issue #33: «до 6 значащих цифр»). */
const PRICE_SIGNIFICANT = 6;

/**
 * Процент коротко. Принимает: строку Decimal или число; null/undefined — нет значения.
 * Возвращает: «0.512» (без знака %) или '—'.
 */
export function fmtPct(x) {
  if (x == null || x === '') return '—';
  const n = Number(x);
  return Number.isFinite(n) ? n.toFixed(PCT_DIGITS) : String(x);
}

const priceFmt = new Intl.NumberFormat('en-US', {
  maximumSignificantDigits: PRICE_SIGNIFICANT,
  useGrouping: false,
});

/**
 * Цена коротко: до 6 значащих цифр, без экспоненты. Принимает: строку Decimal или число.
 * Возвращает: строку или '—'.
 */
export function fmtPrice(x) {
  if (x == null || x === '') return '—';
  const n = Number(x);
  return Number.isFinite(n) ? priceFmt.format(n) : String(x);
}

/** Адрес коротко: «0x1234…abcd». Принимает: строку или null. Возвращает: строку ('' — адреса нет). */
export const shortAddr = (c) => (c ? `${c.slice(0, 6)}…${c.slice(-4)}` : '');

/** Время коротко — только часы:минуты:секунды по-местному. Принимает: строку RFC 3339. */
export const clock = (t) => (t ? new Date(t).toLocaleTimeString('ru-RU') : '—');

/**
 * Причина для чтения: длинный текст ядра, где причины по пулам и площадкам разделены «; » или переводом
 * строки, — массив строк (каждая причина с новой строки). Смысл текста не разбирается.
 * Принимает: строку или null. Возвращает: массив строк (пустой — причины нет).
 */
export const reasonLines = (s) =>
  s ? String(s).split(/;\s+|\n/).map((l) => l.trim()).filter(Boolean) : [];
