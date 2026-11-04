// Файл: ui/src/lib/uncappedLabels.js
// Что делает: подписи расчёта «без потолка» (решение 141): что ограничило объём (коды core/uncapped_types.rs
//   Record.limited_by), округление денег для экрана, период «сутки / 7 дней / свой» в RFC 3339.
// Кто вызывает: pages/Uncapped.svelte, components/UncappedLine.svelte, components/DealUncapped.svelte.
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
