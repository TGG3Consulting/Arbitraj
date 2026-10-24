// Файл: ui/src/lib/coinLabels.js
// Что делает: показ типов проверки монет и пулов из src/core/types/ (check.rs, pool.rs, market.rs):
//   монета (Asset), состояние проверки (CheckState), налоги (TokenTaxes), влияние крючка (HookImpact),
//   причина чёрного списка (BlockReason), группа крючка (HookGroup). Формат — как их serde (enum: строка
//   или {вариант: данные}); числа Decimal приходят строками и показываются без пересчёта.
// Кто вызывает: components/CoinRow.svelte, pages/Manual.svelte, pages/GasStats.svelte.
// От чего зависит: ничего.
// Настройки: нет.

/** Монета: «CAKE» и сокращённый адрес. Принимает: Asset. Возвращает: строку. */
export function assetLabel(a) {
  if (!a) return '';
  const c = a.bsc_contract;
  return c ? `${a.ticker} · ${c.slice(0, 6)}…${c.slice(-4)}` : `${a.ticker} · адрес не сообщён`;
}

/**
 * Состояние проверки (CheckState<T>) для показа.
 * Принимает: значение serde и функцию показа T. Возвращает: {text, cls} — cls для цвета.
 */
export function checkState(s, show) {
  if (s === 'NotChecked') return { text: 'не проверено — торговли нет', cls: 'unset' };
  if (s?.Known !== undefined) return { text: show(s.Known), cls: '' };
  if (s?.Failed) return { text: `проверка не прошла: ${s.Failed.reason}`, cls: 'err' };
  return { text: JSON.stringify(s), cls: '' };
}

/** Налоги: «покупка 1% · продажа 2% · перевод 0%». Принимает: TokenTaxes. Возвращает: строку. */
export const taxesText = (t) => `покупка ${t.buy_pct}% · продажа ${t.sell_pct}% · перевод ${t.transfer_pct}%`;

/** Влияние крючка: «покупка 0,1% · продажа 0,2%». Принимает: HookImpact. Возвращает: строку. */
export const impactText = (i) => `влияние: покупка ${i.buy_pct}% · продажа ${i.sell_pct}%`;

const TAX = { Buy: 'покупку', Sell: 'продажу', Transfer: 'перевод' };

/** Причина чёрного списка. Принимает: BlockReason. Возвращает: строку по-русски. */
export function blockReason(r) {
  if (r === 'NotChecked') return 'проверка налогов ещё не закончена';
  if (r?.CheckFailed) return `проверка не прошла: ${r.CheckFailed.reason}`;
  if (r?.TaxAboveLimit) {
    const t = r.TaxAboveLimit;
    return `налог на ${TAX[t.tax] ?? t.tax} ${t.pct}% выше порога ${t.limit}%`;
  }
  if (r?.Manual) return `ручной чёрный список${r.Manual.note ? `: ${r.Manual.note}` : ''}`;
  return JSON.stringify(r);
}

const GROUP = {
  A: 'A — не трогает обмен и комиссию',
  B: 'B — меняет комиссию или вызывается вокруг обмена',
  C: 'C — может менять сумму обмена',
};

/** Группа крючка (решение 32). Принимает: HookGroup. Возвращает: строку. */
export const hookGroupLabel = (g) => GROUP[g] ?? g;

/** Ссылка на пул: адрес контракта или PoolId в менеджере. Принимает: PoolRef. Возвращает: строку. */
export function poolRefText(p) {
  if (p?.Contract) return p.Contract;
  if (p?.Singleton) return `${p.Singleton.id} в ${p.Singleton.manager}`;
  return JSON.stringify(p);
}

const EXCLUSION = {
  NoBscNetwork: 'нет сети BSC',
  NoContract: 'сеть BSC есть, адреса контракта нет',
  NoSpotBook: 'нет стакана: пары нет на споте или стакан ещё не пришёл',
  NoApiTrading: 'пара не торгуется через API (apiStateBuy/Sell)',
  NoUsdtAddress: 'неизвестен адрес USDT в BSC',
  NoUsdtPool: 'нет пула с USDT',
  PoolsLoading: 'пулы найдены и проверены, состояние ещё загружается (решение 61)',
};

/**
 * Причина отсева монеты на площадке (core/registry.rs, Exclusion). Принимает: строку или {вариант: данные}.
 * Возвращает: текст по-русски.
 */
export function exclusionText(e) {
  if (typeof e === 'string') return EXCLUSION[e] ?? e;
  if (e?.PairsStale !== undefined) return `справочнику пар не верим: ${e.PairsStale}`;
  if (e?.PairNotTrading !== undefined) return `пара не торгуется (status ${e.PairNotTrading})`;
  if (e?.PoolsExcluded) return `все пулы исключены: ${e.PoolsExcluded.join('; ')}`;
  if (e?.IntermediateNotConfigured) return `промежуточные монеты (решение 52) не заданы: ${e.IntermediateNotConfigured}`;
  return JSON.stringify(e);
}

/**
 * Что закрыто у монеты на площадке, где она есть (presence без причин отсева): ввод, вывод, покупка или продажа
 * через API (core/registry.rs Presence: can_receive, can_send, can_buy, can_sell). Направления с закрытой стороной
 * не считаются. Принимает: элемент presence. Возвращает: массив строк (пусто — всё открыто).
 */
export function limitsText(p) {
  const out = [];
  if (!p.can_receive) out.push('ввод закрыт');
  if (!p.can_send) out.push('вывод закрыт');
  if (p.can_buy === false) out.push('покупка через API закрыта (apiStateBuy)');
  if (p.can_sell === false) out.push('продажа через API закрыта (apiStateSell)');
  return out;
}
