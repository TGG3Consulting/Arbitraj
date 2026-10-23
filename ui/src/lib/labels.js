// Файл: ui/src/lib/labels.js
// Что делает: русские подписи для вариантов типов бэкенда — вид значения (Kind), ограничение (Limit),
//   вид области (ScopeKind) из src/settings/schema.rs, уровень журнала (src/storage/journal.rs),
//   варианты выбора настроек (SettingDef.choices из src/settings/catalog/*), вид пула (PoolKind),
//   способ отправки (SendMethod) и состояние канала (ChannelState) из src/core/types/.
//   Это перевод названий, а не значения настроек: сами варианты и границы приходят из схемы бэкенда.
//   Неизвестный вариант показывается как есть.
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
  Choice: 'выбор из списка',
  Address: 'адрес BSC (0x…)',
};
const LIMIT = {
  None: '',
  Positive: 'больше нуля',
  NonNegative: 'ноль или больше',
  Fraction: 'доля от 0 до 1',
  Percent: 'процент от 0 до 100',
};
const SCOPE = {
  Global: 'вся система',
  Venue: 'площадка',
  Coin: 'монета',
  Event: 'событие',
  Dex: 'DEX / вид пула',
  Channel: 'канал отправки',
};
// Варианты выбора — по решениям 29, 32, 34, 37, 43 (docs/DECISIONS.md).
const CHOICE = {
  direct: 'прямой',
  smart: 'умный',
  single: 'одиночная',
  bundle: 'бандл',
  per_trade: 'в каждой сделке',
  separate: 'отдельно по порогу',
  wait: 'ожидание',
  loss_limit: 'порог убытка',
  relay: 'перегон',
};
const POOL_KIND = { V2: 'V2', V3: 'V3', Cl: 'V4 CL (концентрированная)', Bin: 'V4 Bin (бины)' };
const METHOD = { Single: 'одиночная', Bundle: 'бандл' };
const LEVEL = { Info: 'инфо', Warn: 'внимание', Error: 'ошибка' };

/** Подпись вида значения. Принимает: Kind. Возвращает: строку. */
export const kindLabel = (k) => KIND[k] ?? k;
/** Подпись ограничения. Принимает: Limit. Возвращает: строку ('' — без ограничения). */
export const limitLabel = (l) => LIMIT[l] ?? l;
/** Подпись вида области. Принимает: ScopeKind. Возвращает: строку. */
export const scopeLabel = (s) => SCOPE[s] ?? s;
/** Подпись варианта выбора. Принимает: вариант из схемы. Возвращает: «по-русски (вариант)» или вариант. */
export const choiceLabel = (c) => (CHOICE[c] ? `${CHOICE[c]} (${c})` : c);
/** Подпись вида пула. Принимает: PoolKind. Возвращает: строку. */
export const poolKindLabel = (k) => POOL_KIND[k] ?? k;
/** Подпись способа отправки. Принимает: SendMethod. Возвращает: строку. */
export const methodLabel = (m) => METHOD[m] ?? m;

/**
 * Состояние канала (ChannelState: строка или {вариант: {reason}}) для показа.
 * Принимает: значение serde. Возвращает: {text, ok} — ok: true только для Ready.
 */
export function channelState(s) {
  if (s === 'Ready') return { text: 'готов', ok: true };
  if (s === 'Disabled') return { text: 'выключен (channel.enabled)', ok: false };
  if (s === 'NoKey') return { text: 'нет ключа — канал не используется', ok: false };
  if (s && typeof s === 'object') {
    const [k, v] = Object.entries(s)[0];
    const what = k === 'NotConfigured' ? 'не настроен' : k === 'Failing' ? 'отказывает' : k;
    return { text: `${what}: ${v?.reason ?? ''}`, ok: false };
  }
  return { text: String(s), ok: false };
}

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
