// Файл: ui/src/lib/coinFilter.js
// Что делает: итог отбора монеты и фильтры экрана «Монеты» (Issue #33) — только по структурированным полям
//   GET /api/coins (core/coins_view.rs), без разбора русского текста:
//   directions (прошла), presence[].excluded (Exclusion из core/registry.rs), block.reason (BlockReason из
//   core/types/check.rs), taxes (CheckState), problem, presence[].can_send / can_receive. «Крючок» — только по
//   PoolsExcluded (исключены все пулы монеты); исключённые пулы при допущенных — в окне монеты, не в итоге.
// Кто вызывает: pages/Coins.svelte, components/CoinTable.svelte, components/CoinFilters.svelte.
// От чего зависит: ничего (чистые функции над данными ядра).
// Настройки: нет.

/**
 * Итоги отбора: код → подпись. Порядок — порядок в списке фильтра и в строке.
 * Коды — свои у интерфейса; откуда каждый берётся — в reasons().
 */
export const RESULTS = [
  ['passed', 'прошла'],
  ['no_bsc', 'нет сети BSC'],
  ['no_contract', 'нет адреса контракта'],
  ['no_book', 'нет стакана'],
  ['no_usdt_pool', 'нет пула с USDT'],
  ['pools_loading', 'пулы найдены, загружаются'],
  ['hook', 'крючок'],
  ['tax', 'налог'],
  ['blacklist', 'в чёрном списке'],
  ['check_failed', 'проверка не прошла'],
  ['not_checked', 'не проверено'],
  ['problem', 'не оценить'],
  ['closed', 'вывод/депозит закрыт'],
  ['other', 'нет направлений'],
];
const LABEL = Object.fromEntries(RESULTS);
/** Подпись итога. Принимает: код. Возвращает: строку. */
export const resultLabel = (k) => LABEL[k] ?? k;

/** Exclusion (строка или {вариант: данные}) → код итога. */
function exclusionCode(e) {
  if (e === 'NoBscNetwork') return 'no_bsc';
  if (e === 'NoContract') return 'no_contract';
  if (e === 'NoSpotBook') return 'no_book';
  if (e === 'NoUsdtPool' || e === 'NoUsdtAddress' || e?.IntermediateNotConfigured !== undefined) return 'no_usdt_pool';
  if (e === 'PoolsLoading') return 'pools_loading';
  // «Крючок» — только когда исключены ВСЕ пулы монеты (PoolsExcluded); исключённый пул при допущенных — не причина.
  if (e?.PoolsExcluded) return 'hook';
  return 'other';
}

/** BlockReason → код итога. */
function blockCode(r) {
  if (r === 'NotChecked') return 'not_checked';
  if (r?.CheckFailed) return 'check_failed';
  if (r?.TaxAboveLimit) return 'tax';
  if (r?.Manual) return 'blacklist';
  return 'other';
}

/**
 * Итог отбора монеты. Принимает: элемент GET /api/coins.
 * Возвращает: массив кодов RESULTS без повторов — ['passed'], если у монеты есть рабочие направления,
 *   иначе все найденные причины (может быть несколько; ничего не нашлось — ['other']).
 */
export function reasons(c) {
  if ((c.directions ?? []).length) return ['passed'];
  const out = new Set();
  for (const p of c.presence ?? []) {
    for (const e of p.excluded) out.add(exclusionCode(e));
    if (!p.excluded.length && (!p.can_send || !p.can_receive)) out.add('closed');
  }
  if (c.block) out.add(blockCode(c.block.reason));
  if (c.taxes === 'NotChecked') out.add('not_checked');
  if (c.taxes?.Failed) out.add('check_failed');
  if (c.problem) out.add('problem');
  if (!out.size) out.add('other');
  return RESULTS.map(([k]) => k).filter((k) => out.has(k));
}

/** Состояние налогов коротко: 'known' | 'not_checked' | 'failed'. Принимает: CheckState. */
export const taxState = (t) => (t?.Known !== undefined ? 'known' : t?.Failed ? 'failed' : 'not_checked');

/**
 * Значения для списков фильтров — только из данных. Принимает: монеты.
 * Возвращает: {venues: [id], kinds: [PoolKind]} по алфавиту.
 */
export function options(coins) {
  const venues = new Set();
  const kinds = new Set();
  for (const c of coins) {
    for (const p of c.presence ?? []) venues.add(p.venue);
    for (const p of c.pools) kinds.add(p.kind);
  }
  return { venues: [...venues].sort(), kinds: [...kinds].sort() };
}

/**
 * Подходит ли монета под фильтры. Принимает: монету и f = {text, result, venue, pools, kind, tax, onlyBlocked};
 * пустое значение поля — без фильтра. pools: 'yes' | 'no'; tax: 'known' | 'not_checked' | 'failed'.
 * Площадка — монета есть в справочнике этой площадки (presence), с причиной отсева или без.
 * Возвращает: bool.
 */
export function matches(c, f) {
  const q = f.text.trim().toLowerCase();
  if (q && !c.asset.ticker.toLowerCase().includes(q) && !(c.asset.bsc_contract ?? '').toLowerCase().includes(q)) return false;
  if (f.result && !reasons(c).includes(f.result)) return false;
  if (f.venue && !(c.presence ?? []).some((p) => p.venue === f.venue)) return false;
  if (f.pools === 'yes' && !c.pools.length) return false;
  if (f.pools === 'no' && c.pools.length) return false;
  if (f.kind && !c.pools.some((p) => p.kind === f.kind)) return false;
  if (f.tax && taxState(c.taxes) !== f.tax) return false;
  if (f.onlyBlocked && !c.block) return false;
  return true;
}
