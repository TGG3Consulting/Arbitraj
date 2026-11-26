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
  Url: 'адрес https/wss (http/ws — только 127.0.0.1, ::1, localhost)',
  TextList: 'список',
  Ip: 'IP-адрес',
  Choice: 'выбор из списка',
  Address: 'адрес BSC (0x…)',
  AddressList: 'список адресов BSC',
};
const LIMIT = {
  None: '',
  Positive: 'больше нуля',
  NonNegative: 'ноль или больше',
  Fraction: 'доля от 0 до 1',
  Percent: 'процент от 0 до 100',
  PercentOpen: 'процент больше 0 и меньше 100',
  PercentBelow100: 'процент от 0 и меньше 100',
};
const SCOPE = {
  Global: 'вся система',
  Venue: 'площадка',
  Coin: 'монета',
  Event: 'событие',
  Dex: 'DEX / вид пула',
  Channel: 'канал отправки',
  VenueDir: 'площадка + тип направления',
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
  v2_fork: 'форк V2',
  v2_fork_executor: 'форк V2 без роутера (через исполнитель)',
  v3_fork: 'форк V3 (ABI Uniswap)',
  v3_fork_uniswap_lm: 'форк V3 (ABI Uniswap с lmPool, Unchain X)',
  v3_fork_pancake: 'форк V3 (ABI PancakeSwap)',
  v3_fork_ramses: 'Ramses CL (форк V3: currentFee, FeeAdjustment, ramsesV2SwapCallback; решение 483)',
  solidly_v1: 'Solidly V1 (THENA V1)',
  slipstream_dynamic_fee: 'Aerodrome Slipstream (Base, комиссия пула на блоке)',
  slipstream_ramses: 'Ramses CL V2 (Slipstream: свои slot0 / ticks / Mint, FeeAdjustment; решение 483)',
  solidly_v1_ramses: 'Ramses Legacy V2 (Solidly V1: getAmountOut − 1, комиссия fee() / 1e6; решение 483)',
  curve_stableswap: 'StableSwap (Curve 2020, Ellipsis)',
  curve_stableswap_ng: 'Curve StableSwap-NG (plain-пул фабрики NG)',
  curve_stableswap_plain: 'Curve StableSwap 2021 (2pool, plain-пулы старой фабрики)',
  curve_stableswap_meta: 'Curve StableSwap 2021 — метапулы фабрики (над 2pool / fraxbp)',
  curve_stableswap_oracle: 'Curve StableSwap с курсом оракула и ETH (ETH/wstETH; решение 481)',
  curve_twocrypto_ng: 'Curve Twocrypto-NG (crypto-пул из 2 монет)',
  curve_tricrypto_ng: 'Curve Tricrypto-NG (crypto-пул из 3 монет)',
  curve_crypto_v1: 'Curve crypto v1 фабрики (crypto-пул из 2 монет)',
  curve_tricrypto_v1: 'Curve tricrypto 2021 (crypto-пул из 3 монет, без фабрики)',
  curve_crypto2_v1: 'Curve crypto 2021 из 2 монет без фабрики (EURS/2CRV)',
  nomiswap_stable: 'Nomiswap Stable (стейбл-пара, комиссия со входа)',
  metric_omm: 'Metric OMM (цена провайдера, лесенка ответов пула)',
  iziswap: 'iZiSwap (точки и лимитные ордера, лесенка ответов пула)',
  biswap_v3: 'Biswap V3 (форк iZiSwap, лесенка ответов пула)',
  kyber_dmm: 'KyberSwap Classic DMM (усиление amp, динамическая комиссия)',
  smardex: 'SmarDex (фиктивные резервы, средняя цена по времени блока)',
  fluid_t1: 'Fluid DEX T1 (смарт-залог и смарт-долг, мнимые резервы)',
  basin_well: 'Basin (скважины Beanstalk: ConstantProduct2 / Stable2, без комиссии; решение 485)',
  shell_proteus: 'Shell Protocol Proteus (кусочная гипербола, учёт Ocean)',
  lb_v21: 'Liquidity Book V2.1 (LFJ / Trader Joe: бины, изменчивая комиссия)',
  lb_v20: 'Liquidity Book v2.0 (LFJ V2 / Trader Joe V2: бины, изменчивая комиссия; решение 445)',
  lb_v22: 'Liquidity Book V2.2 (LFJ: бины, изменчивая комиссия, крючки пула)',
  ekubo: 'Ekubo (синглтон-ядро, тики 1e-6, расширение MEVCapture)',
  balancer_v2: 'Balancer V2 (хранилище Vault, весовые и стейблсвап-пулы)',
  balancer_v3: 'Balancer V3 (хранилище Vault V3, весовые и стейблсвап-пулы фабрик, без крючков обмена)',
  camelot_v2: 'Camelot V2 (направленная комиссия, stable / volatile пары)',
  aero_basic: 'Aero Basic (VolatilePool / StablePool, MEV-налог модуля)',
  pancake_stableswap: 'StableSwap PancakeSwap',
  lista_stableswap: 'StableSwap Lista (с оракулом)',
  saddle_stableswap: 'StableSwap Saddle (Nerve)',
  wombat: 'Wombat Exchange (покрытие cash / liability, haircut; решение 484)',
  woofi_v2: 'WooFi WooPPV2 (лесенка цен пула)',
  maverick_v2: 'Maverick V2 (тики-бины, кривая квотера)',
  pancake_infinity_stable: 'PancakeSwap Infinity Stable (StableSwap-NG в крючке CL)',
  pancakeswap_infinity_cl: 'PancakeSwap Infinity CL',
  pancakeswap_infinity_bin: 'PancakeSwap Infinity Bin',
  uniswap_v4: 'Uniswap V4',
  unsupported: 'не поддерживается',
  fixed: 'одна на DEX',
  pool: 'своя у пары',
  // Решение 273: dex.solidly_fee_source (pool — «своя у пары», выше) и dex.solidly_fee_rule.
  factory_pool: 'фабрика: getFee(pool, stable)',
  factory_stable: 'фабрика: getFee(stable)',
  factory_by_type: 'фабрика: метод по типу пула',
  numerator: 'числитель над делителем',
  divisor: 'делитель (вход / комиссия)',
  pancakeswap_infinity: 'Universal Router PancakeSwap Infinity',
};
// Подписи варианта, у которого смысл зависит от настройки (один вариант — разные настройки): `pool` у dex.v2_fee_source —
// «своя у пары» (общая подпись выше), у dex.v3_fee_source (решение 274) — fee() пула.
const CHOICE_BY_SETTING = {
  'dex.v3_fee_source': {
    pool: 'fee() пула',
    factory_swap_fee: 'комиссия от фабрики swapFee (SheepDex)',
    pool_fee_each_read: 'fee() пула при каждом чтении (SquadSwap V3)',
  },
  // Решение 433: модель газа сети площадки DEX.
  'dex.gas_mode': {
    legacy: 'legacy — одна цена gasPrice (BSC)',
    eip1559_l1: 'EIP-1559 + газ L1 (Arbitrum)',
  },
  // Решение 302: запасной источник пополнения в симуляции.
  'presell.fund_fallback': {
    singleton: 'держатель-синглтон (Uniswap V4 PoolManager / Infinity Vault)',
  },
};
const POOL_KIND = {
  V2: 'V2',
  V3: 'V3',
  Cl: 'V4 CL (концентрированная)',
  Bin: 'V4 Bin (бины)',
  StableSwap: 'StableSwap',
  OraclePmm: 'PMM от оракула',
  TickBin: 'Тики-бины (Maverick V2)',
  AmpV2: 'V2 с усилением (KyberSwap DMM)',
  Fictive: 'Фиктивные резервы (SmarDex)',
  LbBin: 'Бины с изменчивой комиссией (Liquidity Book V2.1)',
  Ekubo: 'Пул в ядре Ekubo',
  VaultPool: 'Пул в хранилище (Balancer V2)',
  SmartColDebt: 'Смарт-залог и смарт-долг (Fluid DEX T1)',
  BasinWell: 'Скважина Basin (функция скважины ConstantProduct2 / Stable2)',
  Proteus: 'Кусочная гипербола за учётом Ocean (Shell Protocol Proteus)',
  Crypto: 'Crypto-пул Curve (Twocrypto / Tricrypto)',
};
const METHOD = { Single: 'одиночная', Bundle: 'бандл' };
const LEVEL = { Info: 'инфо', Warn: 'внимание', Error: 'ошибка' };

/** Подпись вида значения. Принимает: Kind. Возвращает: строку. */
export const kindLabel = (k) => KIND[k] ?? k;
/** Подпись ограничения. Принимает: Limit. Возвращает: строку ('' — без ограничения). */
export const limitLabel = (l) => LIMIT[l] ?? l;
/** Подпись вида области. Принимает: ScopeKind. Возвращает: строку. */
export const scopeLabel = (s) => SCOPE[s] ?? s;
/**
 * Подпись варианта выбора. Принимает: вариант из схемы и (необязательно) имя настройки — у неё может быть своя подпись
 * варианта (CHOICE_BY_SETTING). Возвращает: «по-русски (вариант)» или вариант.
 */
export const choiceLabel = (c, name) => {
  const t = CHOICE_BY_SETTING[name]?.[c] ?? CHOICE[c];
  return t ? `${t} (${c})` : c;
};
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
