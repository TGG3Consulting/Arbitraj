// Файл: ui/src/lib/liveGrid.js
// Что делает: раскладка «Живого экрана» по монетам (решение 55): из списка SpreadView (по направлениям)
//   собирает строки «одна монета» и колонки «пара площадок в обе стороны». Пары берутся только из
//   площадок, которые реально есть в данных, — имён площадок в коде нет; третья площадка сама даёт новые пары.
// Кто вызывает: pages/Live.svelte, components/LiveCoins.svelte.
// От чего зависит: ничего (чистые функции над данными ядра, формат — src/api/README.md).
// Настройки: нет.

/** Состояния, при которых спред выше порога комиссий (signal.rs State). */
const ACTIVE = ['Holding', 'Candidate', 'Anomaly'];

/**
 * Ключ монеты: тикер + адрес контракта (одинаковый тикер бывает разными токенами).
 * Принимает: Asset {ticker, bsc_contract}. Возвращает: строку.
 */
export const coinKey = (a) => `${a.ticker}|${a.bsc_contract}`;

/**
 * Ключ направления внутри строки монеты: «купить на A → продать на B».
 * Принимает: id площадки покупки и продажи. Возвращает: строку.
 */
export const legKey = (buy, sell) => `${buy}>${sell}`;

/**
 * Колонки таблицы: для каждой пары площадок из данных — два направления (A → B, затем B → A).
 * Площадки упорядочены по id, чтобы колонки не прыгали между обновлениями.
 * Принимает: SpreadView[]. Возвращает: [{a, b, legs: [{buy, sell, key}, {buy, sell, key}]}].
 */
export function venuePairs(views) {
  const pairs = new Map();
  for (const v of views) {
    const [a, b] = [v.direction.buy_on, v.direction.sell_on].sort();
    const id = `${a}|${b}`;
    if (!pairs.has(id)) {
      pairs.set(id, {
        a,
        b,
        legs: [
          { buy: a, sell: b, key: legKey(a, b) },
          { buy: b, sell: a, key: legKey(b, a) },
        ],
      });
    }
  }
  return [...pairs.values()].sort((x, y) => (`${x.a}|${x.b}` < `${y.a}|${y.b}` ? -1 : 1));
}

/** Спред направления числом; нет спреда — null. */
const pct = (v) => (v?.spread_pct == null ? null : Number(v.spread_pct));

/**
 * Строки монет. Принимает: SpreadView[].
 * Возвращает: [{key, asset, legs: {legKey: SpreadView}, best: число | null, active: bool, at}] —
 *   best — наибольший спред из направлений монеты, active — хоть одно направление выше порога,
 *   at — самое свежее обновление.
 */
export function coinRows(views) {
  const rows = new Map();
  for (const v of views) {
    const d = v.direction;
    const k = coinKey(d.asset);
    if (!rows.has(k)) rows.set(k, { key: k, asset: d.asset, legs: {}, best: null, active: false, at: v.at });
    const r = rows.get(k);
    r.legs[legKey(d.buy_on, d.sell_on)] = v;
    const p = pct(v);
    if (p != null && (r.best == null || p > r.best)) r.best = p;
    if (ACTIVE.includes(v.state)) r.active = true;
    if (v.at > r.at) r.at = v.at;
  }
  return [...rows.values()];
}

/**
 * Сортировка по умолчанию: наибольший спред из двух (и более) направлений, монеты без спреда — в конце,
 * при равенстве — по тикеру. Принимает: строки coinRows. Возвращает: новый массив.
 */
export const byBestSpread = (rows) =>
  [...rows].sort((x, y) => {
    if (x.best == null && y.best == null) return x.asset.ticker.localeCompare(y.asset.ticker);
    if (x.best == null) return 1;
    if (y.best == null) return -1;
    return y.best - x.best || x.asset.ticker.localeCompare(y.asset.ticker);
  });
