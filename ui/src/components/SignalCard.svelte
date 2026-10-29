<!--
  Файл: ui/src/components/SignalCard.svelte
  Что делает: полная карточка сигнала (решение 74, core/card.rs): монета и контракт («Копировать»), где купить и где
    продать, количество и средние цены, стакан биржи (лучшая цена, USDT в полосе ±dex.depth_band_pct, сколько уровней,
    «полоса шире загруженных уровней»), пулы маршрута DEX (глубина по стороне сделки, полоса, мелкий/глубокий, блок),
    маршрут DEX на сделку (режим, доли сплита, выход и amountOutMin на ногу, «исполним как есть: да/нет» + причина —
    решения 80, 84) и, в граф-режиме, «лучший найденный маршрут» (сигнал, если он лучше, но неисполним как есть),
    прибыль на бюджете, без бюджета — максимальный объём и его прибыль, максимальная прибыль и чем ограничен поиск,
    ссылки на пару и пул или «шаблон ссылки не задан». Метка «СУХОЙ» у сухого сигнала.
  Кто вызывает: pages/Live.svelte (раскрытие прошедшего кандидата), pages/Episodes.svelte (карточка в записи).
  От чего зависит: components/CopyAddr.svelte, lib/labels.js (вид пула).
  Настройки: нет (card.max_search_usdt, venue.pair_url_template, dex.pool_url_template — экран «Настройки»).
-->
<script>
  import CopyAddr from './CopyAddr.svelte';
  import { poolKindLabel } from '../lib/labels.js';

  let { card } = $props();
  const max = $derived(card.max?.Ok);
  const maxErr = $derived(card.max?.Err);
  const sideText = (s) => (s === 'Buy' ? 'покупка' : 'продажа');
  const routeMode = (m) => (m === 'Graph' ? 'граф' : m === 'Smart' ? 'умный' : 'прямой');
  const pct = (s) => (Number(s) * 100).toFixed(2).replace(/\.?0+$/, '');
  const legHops = (lg) => lg.hops.map((h) => `${poolKindLabel(h.kind)} ${h.pool}`).join(' → ');
</script>

<div class="signal">
  <p>
    {#if card.dry}<span class="tag dry">СУХОЙ</span>{/if}
    <strong>{card.ticker}</strong>
    {#if card.contract}<CopyAddr addr={card.contract} />{:else}<span class="muted">контракт неизвестен</span>{/if}
  </p>
  <p>Купить на <strong>{card.buy_on}</strong> → продать на <strong>{card.sell_on}</strong></p>
  <p>Купить {card.qty} по {card.buy_avg} (средняя); продать {card.sell_qty} по {card.sell_avg}</p>
  {#each card.books as b, i (i)}
    <p>
      Стакан <code>{b.venue}</code> ({b.side === 'Buy' ? 'asks' : 'bids'}):
      {#if b.error}<span class="err">{b.error}</span>
      {:else}лучшая {b.best}; в полосе ±{b.band_pct}% — <strong>{b.usd_in_band} USDT</strong> ({b.qty_in_band} монет,
        уровней загружено {b.levels}){#if b.beyond}<span class="warn-text"> — полоса шире загруженных уровней (venue.depth_levels)</span>{/if}{/if}
    </p>
  {/each}
  {#each card.pools as p (p.pool)}
    <p>
      Пул <code>{p.venue}</code> {poolKindLabel(p.kind)} <code>{p.pool}</code>, {sideText(p.side)}:
      {#if p.depth_usd != null}глубина <strong>{p.depth_usd} USDT</strong> в полосе ±{p.band_pct}%
        {#if !p.deep}<span class="warn-text">(мелкий)</span>{/if}{#if !p.full}<span class="warn-text">, полоса прочитана не целиком</span>{/if}
      {:else}<span class="err">{p.error ?? 'глубина не посчитана'}</span>{/if}
      <span class="muted">· блок {p.block}</span>
    </p>
  {/each}
  {#if card.route}
    <p>
      Маршрут на сделку (<strong>{routeMode(card.route.mode)}</strong>, {sideText(card.route.side)}): исполним как есть —
      {#if card.route.executable}<strong>да</strong>{:else}<span class="err">нет</span> <span class="warn-text">({card.route.exec_note})</span>{/if}
    </p>
    {#each card.route.legs as lg, i (i)}
      <p class="leg">доля {pct(lg.share)}%: {legHops(lg)} — выход {lg.expected_out}, amountOutMin <strong>{lg.amount_out_min}</strong></p>
    {/each}
  {/if}
  {#if card.signal_route}
    <p class="warn-text">
      Лучший найденный маршрут (сигнал, на сделку НЕ идёт — {card.signal_route.exec_note ?? 'неисполним как есть'}; на сделку — исполнимый выше):
    </p>
    {#each card.signal_route.legs as lg, i (i)}
      <p class="leg muted">доля {pct(lg.share)}%: {legHops(lg)} — выход {lg.expected_out}</p>
    {/each}
  {/if}
  <p>Прибыль на бюджете {card.budget ?? '—'} USDT: <strong>{card.profit_usd} USDT</strong> ({card.profit_pct}%)</p>
  {#if max}
    <p>
      Без бюджета: наибольший найденный объём <strong>{max.max_qty ?? '—'}</strong> (прибыль {max.max_profit ?? '—'}, списание {max.max_spend ?? '—'} USDT);
      максимальная прибыль <strong>{max.opt_profit} USDT</strong> на {max.opt_qty}
    </p>
    <p class="muted">Наибольший объём ищется бисекцией: при нескольких участках прибыльности дальний может быть пропущен.</p>
    <p class="muted">Край поиска: {max.limit} (предел card.max_search_usdt = {max.cap}, наибольший возможный объём {max.upper_qty}, расчётов {max.evals})</p>
  {:else}
    <p class="muted">Без бюджета: не посчитано — {maxErr}</p>
  {/if}
  {#each card.links as l, i (i)}
    <p>
      {l.kind === 'pair' ? 'Пара' : 'Пул'} <code>{l.venue}</code> {l.label}:
      {#if l.url}<a href={l.url} target="_blank" rel="noopener noreferrer">{l.url}</a>
      {:else}<span class="unset">{l.why}</span>{/if}
    </p>
  {/each}
</div>

<style>
  .signal p { margin: 4px 0; }
  .signal p.leg { margin: 2px 0 2px 16px; font-size: 0.95em; }
  .tag.dry { background: #b45309; color: #fff; border-color: #b45309; font-weight: 700; }
</style>
