// Файл: ui/src/lib/uncappedLabels.js
// Что делает: подписи расчёта «без потолка» (решение 141): что ограничило объём (коды core/uncapped_types.rs
//   Record.limited_by), округление денег для экрана, период «сутки / 7 дней / свой» в RFC 3339.
// Решение 197: вердикт расчёта на нашем бюджете по-русски (слова «бой» на экране нет), длительность окна.
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

/** Окно записи, с: сделка — от начала окна (battle.window_from) до расчёта после покупки (at); кандидат — эпизод
 *  (ep.first_at → ep.last_at). null — у сделки начала окна нет. */
export function windowSec(b) {
  if (b.battle?.verdict === 'deal') {
    return b.battle.window_from ? (Date.parse(b.at) - Date.parse(b.battle.window_from)) / 1000 : null;
  }
  return b.ep ? (Date.parse(b.ep.last_at) - Date.parse(b.ep.first_at)) / 1000 : null;
}
