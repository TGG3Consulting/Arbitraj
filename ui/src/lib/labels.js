// Файл: ui/src/lib/labels.js
// Что делает: русские подписи для вариантов типов бэкенда — вид значения (Kind), ограничение (Limit),
//   вид области (ScopeKind) из src/settings/schema.rs, уровень журнала (src/storage/journal.rs).
//   Это перевод названий, а не значения настроек. Неизвестный вариант показывается как есть.
// Кто вызывает: components/SettingCard.svelte, SettingEditor.svelte, pages/Journal.svelte.
// От чего зависит: ничего.
// Настройки: нет.

const KIND = {
  Decimal: 'десятичное число',
  Integer: 'целое число',
  Bool: 'да/нет',
  Text: 'текст',
  Url: 'адрес https/wss',
  TextList: 'список',
  Ip: 'IP-адрес',
};
const LIMIT = {
  None: '',
  Positive: 'больше нуля',
  NonNegative: 'ноль или больше',
  Fraction: 'доля от 0 до 1',
  Percent: 'процент от 0 до 100',
};
const SCOPE = { Global: 'вся система', Venue: 'площадка', Coin: 'монета', Event: 'событие' };
const LEVEL = { Info: 'инфо', Warn: 'внимание', Error: 'ошибка' };

/** Подпись вида значения. Принимает: Kind. Возвращает: строку. */
export const kindLabel = (k) => KIND[k] ?? k;
/** Подпись ограничения. Принимает: Limit. Возвращает: строку ('' — без ограничения). */
export const limitLabel = (l) => LIMIT[l] ?? l;
/** Подпись вида области. Принимает: ScopeKind. Возвращает: строку. */
export const scopeLabel = (s) => SCOPE[s] ?? s;
/** Подпись уровня журнала. Принимает: Level. Возвращает: строку. */
export const levelLabel = (l) => LEVEL[l] ?? l;

/**
 * Значение настройки для показа.
 * Принимает: JSON-значение или undefined. Возвращает: строку; undefined → '' (показывается «не задано»).
 */
export function showValue(v) {
  if (v === undefined || v === null) return '';
  if (v === true) return 'да';
  if (v === false) return 'нет';
  if (Array.isArray(v)) return v.length ? v.join(', ') : '(пустой список)';
  return String(v);
}

/** Время для показа. Принимает: строку RFC 3339. Возвращает: местное время. */
export const showTime = (t) => new Date(t).toLocaleString('ru-RU');
