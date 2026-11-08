// Файл: ui/src/lib/uncappedLabels.js
// Что делает: подписи расчёта «без потолка» (решение 141): что ограничило объём (коды core/uncapped_types.rs
//   Record.limited_by), округление денег для экрана, период «сутки / 7 дней / свой» в RFC 3339.
// Решение 197: вердикт расчёта на нашем бюджете по-русски (слова «бой» на экране нет), длительность окна.
// Решение 217: источник данных записи (basis) и признак «посчитано после покупки — занижено» у старых записей сделок.
// Кто вызывает: pages/Uncapped.svelte, components/UncappedLine.svelte, components/UncappedRecord.svelte,
//   components/DealUncapped.svelte.
// От чего зависит: ничего.
// Настройки: нет.

const LIMITED = {
  per_trade: 'лимит на сделку money.max_per_trade_usd',
  per_coin: 'лимит на монету money.max_per_coin_usd',
  in_transit: 'лимит в пути money.max_in_transit_usd',
  dry: 'виртуальный бюджет dry.budget_usdt',
  free: 'свободный остаток на площадке покупки',
  budget: 'бюджет (составляющие не прочитаны)',
  optimum: 'ничего: оптимум внутри бюджета',
  liquidity: 'ликвидность площадок',
  search_cap: 'предел поиска card.max_search_usdt',
};

/** Что ограничило — по коду. */
export const limitedLabel = (k) => LIMITED[k] ?? k;

/** Бюджет срезал оптимум (как core budget_bound). */
export const budgetBound = (k) => !['optimum', 'liquidity', 'search_cap'].includes(k);

/** Деньги для экрана: до 4 знаков, без хвостовых нулей; null — «—». */
export function money(x) {
  if (x == null) return '—';
  const n = Number(x);
  return Number.isFinite(n) ? String(Math.round(n * 1e4) / 1e4) : String(x);
}

/** Период: 'day' / 'week' — последние сутки / 7 дней до сейчас; 'custom' — from/to из полей datetime-local.
 *  Возвращает: {from, to} в RFC 3339 (пустые — без границы). */
export function period(kind, fromLocal, toLocal) {
  const now = Date.now();
  if (kind === 'day') return { from: new Date(now - 864e5).toISOString(), to: '' };
  if (kind === 'week') return { from: new Date(now - 7 * 864e5).toISOString(), to: '' };
  const iso = (v) => (v ? new Date(v).toISOString() : '');
  return { from: iso(fromLocal), to: iso(toLocal) };
}

const VERDICT = { deal: 'сделка', accept: 'прошёл, сделка не запущена', reject: 'отказ' };

/** Вердикт расчёта на нашем бюджете (Battle.verdict) — по-русски. */
export const verdictLabel = (v) => VERDICT[v] ?? v;

/** Длительность в секундах — «N с», «M мин S с», «H ч M мин»; null — «—». */
export function duration(sec) {
  if (sec == null || !Number.isFinite(Number(sec))) return '—';
  const s = Math.max(0, Math.round(Number(sec)));
  if (s < 60) return `${s} с`;
  if (s < 3600) return `${Math.floor(s / 60)} мин ${s % 60} с`;
  return `${Math.floor(s / 3600)} ч ${Math.floor((s % 3600) / 60)} мин`;
}

/** Окно записи, с: сделка — от начала окна (battle.window_from) до момента решения о входе (decided_at, решение 217;
 *  у старых записей — до расчёта после покупки, at); кандидат — эпизод (ep.first_at → ep.last_at). null — у сделки
 *  начала окна нет. */
export function windowSec(b) {
  if (b.battle?.verdict === 'deal') {
    const end = b.decided_at ?? b.at;
    return b.battle.window_from ? (Date.parse(end) - Date.parse(b.battle.window_from)) / 1000 : null;
  }
  return b.ep ? (Date.parse(b.ep.last_at) - Date.parse(b.ep.first_at)) / 1000 : null;
}

/** Решение 217: запись сделки посчитана после нашей покупки (старая запись без снимка рынка) — цифры занижены.
 *  Поле basis нет — запись до решения 217 («после покупки»; сервер дописывает basis сам, это запас). */
export const understated = (b) => b?.battle?.verdict === 'deal' && (b.basis ?? 'live_after_buy') === 'live_after_buy';

const BASIS = {
  // Решение 237: все входы — на момент входа; decision_snapshot — записи до него (σ и курсы позже входа).
  decision_inputs: 'все данные на момент входа: стаканы, пулы, σ, время в пути, курсы, газ',
  decision_snapshot: 'стаканы на момент входа, σ и курсы — позже (до решения 237, несравнимо)',
  live_after_buy: 'посчитано после покупки — занижено',
  live: 'живые котировки',
  // Решение 218: старая запись сделки пересчитана по окну (основная запись сделки — последняя).
  recomputed_window: 'пересчитано по окну: стаканы на момент решения (до покупки)',
  recomputed_window_dex_plan: 'пересчитано по окну: биржа — стакан на момент решения, DEX — из плана (приблизительно)',
};

/** Решение 237: запись сделки по снимку стаканов, но σ и курсы — на момент расчёта (до решения 237): несравнимо. */
export const lateInputs = (b) => b?.battle?.verdict === 'deal' && b?.basis === 'decision_snapshot';

/** Решение 237: «заработали бы» / «потеряли бы» по знаку расчёта. */
export const wouldText = (profit) => (Number(profit) < 0 ? 'потеряли бы' : 'заработали бы');

/** Решение 218: запись — пересчёт старой записи сделки по окну. */
export const recomputed = (b) => (b?.basis ?? '').startsWith('recomputed_window');

/** Решение 217: на каких данных посчитано (Record.basis) — по-русски. */
export const basisLabel = (k) => BASIS[k] ?? k;
