<!--
  Файл: ui/src/components/LiveCoins.svelte
  Что делает: таблица «Живого экрана» по монетам (решение 55): одна строка — одна монета, прошедшая отбор;
    колонки — каждая пара площадок из данных в обе стороны («купить на A → продать на B» и обратно).
    Сейчас пара одна (две колонки направлений); новая площадка сама добавляет колонки — имён площадок в коде нет.
  Кто вызывает: pages/Live.svelte.
  От чего зависит: lib/liveGrid.js, lib/labels.js (showTime), components/SpreadCell.svelte.
  Настройки: нет.
-->
<script>
  import { venuePairs, coinRows, byBestSpread } from '../lib/liveGrid.js';
  import { showTime } from '../lib/labels.js';
  import SpreadCell from './SpreadCell.svelte';

  /** views — все SpreadView; match(asset) — фильтр по монете; onlyActive — «только выше порога». */
  let { views, match, onlyActive } = $props();

  const pairs = $derived(venuePairs(views));
  const rows = $derived(byBestSpread(coinRows(views).filter((r) => match(r.asset) && (!onlyActive || r.active))));
  const legCount = $derived(pairs.length * 2);

  /** Причина «—» для направления, которого нет в данных ядра. */
  const missing = (r, leg) =>
    `${r.asset.ticker}: направления ${leg.buy} → ${leg.sell} нет в данных ядра — монета не прошла первичный фильтр для этой пары площадок (экран «Монеты»)`;
  /** Короткий адрес контракта для подсказки у тикера. */
  const short = (c) => (c ? `${c.slice(0, 6)}…${c.slice(-4)}` : '');
</script>

<table>
  <thead>
    {#if pairs.length > 1}
      <tr>
        <th></th>
        {#each pairs as p (p.a + p.b)}<th colspan="2">{p.a} ↔ {p.b}</th>{/each}
        <th></th>
      </tr>
    {/if}
    <tr>
      <th>Монета</th>
      {#each pairs as p (p.a + p.b)}
        {#each p.legs as leg (leg.key)}<th>купить на {leg.buy} → продать на {leg.sell}</th>{/each}
      {/each}
      <th>Обновлено</th>
    </tr>
  </thead>
  <tbody>
    {#each rows as r (r.key)}
      <tr>
        <td title={r.asset.bsc_contract}><strong>{r.asset.ticker}</strong><br /><span class="muted small">{short(r.asset.bsc_contract)}</span></td>
        {#each pairs as p (p.a + p.b)}
          {#each p.legs as leg (leg.key)}<SpreadCell v={r.legs[leg.key]} missing={missing(r, leg)} />{/each}
        {/each}
        <td>{showTime(r.at)}</td>
      </tr>
    {:else}
      <tr>
        <td colspan={legCount + 2} class="muted">
          {#if views.length}Нет монет под фильтр.{:else}Монет нет: справочник пуст или ни одна монета ещё не прошла первичный фильтр (экран «Монеты»).{/if}
        </td>
      </tr>
    {/each}
  </tbody>
</table>

<style>
  .small { font-size: 0.85em; }
</style>
