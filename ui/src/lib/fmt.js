// Файл: ui/src/lib/fmt.js
// Что делает: короткий показ чисел, адресов, времени и причин в компактных строках (Issue #33):
//   проценты — 3 знака после точки, цены — до 6 значащих цифр, адрес — «0x1234…abcd», время — только часы.
//   Полные значения показываются как есть во всплывающих окнах (они приходят строками Decimal из ядра).
//   Это правила показа, а не параметры торговли: на расчёты не влияют.
// Кто вызывает: components/SpreadCell.svelte, LiveCoins.svelte, LiveCoinModal.svelte, CoinTable.svelte, pages/Manual.svelte.
//   Решение 164: деньги — 2 знака (меньше 0.01 — 4), количество монеты — 4 значащие цифры (как notify/human.rs).
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

/** Знаков у денег (решение 164); по модулю меньше 0.01 — SMALL_MONEY_DIGITS, чтобы не было «0.00». */
const MONEY_DIGITS = 2;
const SMALL_MONEY_DIGITS = 4;
/** Значащих цифр у количества монеты (решение 164; целая часть показывается целиком). */
const QTY_SIGNIFICANT = 4;

/**
 * Деньги коротко (USDT): «44.98», «−45.02», «0.0041». Принимает: строку Decimal или число. Возвращает: строку или '—'.
 */
export function fmtUsd(x) {
  if (x == null || x === '') return '—';
  const n = Number(x);
  if (!Number.isFinite(n)) return String(x);
  const d = n !== 0 && Math.abs(n) < 0.01 ? SMALL_MONEY_DIGITS : MONEY_DIGITS;
  return n.toFixed(d).replace('-', '−');
}

/**
 * Количество монеты коротко: целая часть целиком, всего не меньше 4 значащих цифр, без хвостовых нулей
 * («139708165163», «44.98», «0.004052»). Принимает: строку Decimal или число. Возвращает: строку или '—'.
 */
export function fmtQty(x) {
  if (x == null || x === '') return '—';
  const n = Number(x);
  if (!Number.isFinite(n)) return String(x);
  const a = Math.abs(n);
  const intDigits = a >= 1 ? Math.floor(a).toString().length : 0;
  const lead = a > 0 && a < 1 ? Math.ceil(-Math.log10(a)) - 1 : 0;
  const dp = intDigits >= QTY_SIGNIFICANT ? 0 : intDigits > 0 ? QTY_SIGNIFICANT - intDigits : lead + QTY_SIGNIFICANT;
  return String(Number(n.toFixed(Math.min(dp, 20)))).replace('-', '−');
}
