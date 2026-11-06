<!--
  Файл: ui/src/components/UncappedRecord.svelte
  Что делает: подробности записи «без потолка» (решение 141): расчёт на нашем бюджете (вердикт, причина, бюджет и его составляющие,
    свободный остаток у сделки), оптимум без потолка (объём, вложение, прибыль, %, средние цены, выручка, затраты, время в
    пути), минимум до плюса, на нашем бюджете, таблица потолков записи, край поиска и расчётов, состав затрат на оптимуме
    (CostLines).
  Кто вызывает: pages/Uncapped.svelte.
  От чего зависит: components/CostLines.svelte, components/UncappedLine.svelte, lib/uncappedLabels.js.
  Настройки: нет.
-->
<script>
  import CostLines from './CostLines.svelte';
  import UncappedLine from './UncappedLine.svelte';
  import { money, verdictLabel } from '../lib/uncappedLabels.js';

  /** rec — Record ядра (core/uncapped_types.rs) как JSON. */
  let { rec } = $props();
  const p = $derived(rec.parts);
</script>

<UncappedLine u={rec} />
<p>
  На нашем бюджете: {verdictLabel(rec.battle.verdict)}{#if rec.battle.reason} — {rec.battle.reason}{/if}{#if rec.battle.dry} <span class="tag">сухой</span>{/if};
  бюджет {money(rec.budget)}{#if p} (на сделку {money(p.per_trade)}, остаток на монету {money(p.coin_left)}, в пути {money(p.transit_left)}{#if p.dry != null}, сухой {money(p.dry)}{/if}){:else if rec.limits_error} <span class="muted">({rec.limits_error})</span>{/if}
  {#if rec.battle.free != null}; перед покупкой предел {money(rec.battle.free)} при бюджете плана {money(rec.battle.plan_budget)}{/if}
</p>
<p>
  Оптимум: {rec.opt.qty} монет, вложить {money(rec.opt.invested)}, прибыль {money(rec.opt.profit)} USDT ({money(rec.opt_profit_pct)}%);
  покупка {rec.opt_buy_avg}, продажа {rec.opt_sell_avg}, выручка {money(rec.opt_proceeds)}, затраты {money(rec.opt_costs_usdt)} USDT,
  T = {rec.opt_transit_s} с; пороги: {rec.min_profit_usd} USDT и {rec.min_profit_pct}%
</p>
<p class="muted">
  Край: {rec.limit} (предел card.max_search_usdt = {rec.search_cap}, наибольший объём {rec.upper_qty}); расчётов {rec.evals},
  пиков {rec.peaks}{#if rec.capped}, <span class="warn-text">исчерпан uncapped.max_evals — итог по посчитанному</span>{/if}; {money(rec.ms)} мс.
  Минимум до плюса — первый проходящий среди посчитанных и бисекция вниз: участок прибыльности уже шага сетки ниже может быть пропущен.
</p>
{#if rec.levels?.length}
  <table>
    <thead><tr><th>потолок</th><th>объём</th><th>вложение</th><th>прибыль</th><th>проходит пороги</th></tr></thead>
    <tbody>
      {#each rec.levels as l (l.cap)}
        <tr><td>{l.cap}</td><td>{l.at?.qty ?? '—'}</td><td>{money(l.at?.invested)}</td><td>{money(l.at?.profit)}</td><td>{l.passes ? 'да' : 'нет'}</td></tr>
      {/each}
    </tbody>
  </table>
{/if}
<details>
  <summary>Состав затрат на оптимуме без потолка</summary>
  <CostLines lines={rec.opt_costs} />
</details>
