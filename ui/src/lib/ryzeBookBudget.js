// Нагрузка фоновой книги для карточки настроек Ryze. Источники: ryze/book.rs, price.rs, budget.rs.
// Один размер даёт до четырёх POST на BUY (exact-output и до трёх exact-input), один — на SELL.
const buyRequests=4, sellRequests=1, minuteMs=60000;
const positiveInteger = n=>Number.isInteger(n) && n>0;
export const ryzeBookSetting = name=>name==='ryze.quote_sizes' || name==='ryze.books_interval_ms';

/** Верхняя оценка на один опрос по заданным строкам. Бюджет всех кошельков на origin берётся только из Global. */
export function ryzeBookBudget(values,venue) {
  const rows=values['ryze.quote_sizes|Venue|'+venue];
  const interval=values['ryze.books_interval_ms|Venue|'+venue];
  const cap=values['ryze.router_requests_per_minute|Global|'];
  const sizes=Array.isArray(rows) ? rows.length : 0;
  const buy=sizes*buyRequests, sell=sizes*sellRequests, total=buy+sell;
  const perMinute=sizes && positiveInteger(interval) && positiveInteger(cap) ? total*minuteMs/interval : null;
  const mayExhaust=perMinute!==null && (perMinute>=cap || total>cap);
  return {sizes,buy,sell,total,perMinute,cap:positiveInteger(cap) ? cap : null,mayExhaust};
}
