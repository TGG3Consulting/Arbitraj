<!--
  Файл: ui/src/components/LiveCoins.svelte
  Что делает: компактная таблица «Живого экрана» по монетам (решение 55, Issue #33): одна строка — одна монета,
    прошедшая отбор; по каждой паре площадок из данных — оба направления («купить на A → продать на B» и обратно),
    у каждого: спред %, порог %, короткое состояние; в конце — время обновления. Строки не переносятся,
    при многих колонках прокручивается только таблица. Щелчок по строке — всплывающее окно с полной
    информацией (LiveCoinModal.svelte), оно обновляется вживую вместе со строкой.
    Имён площадок в коде нет — новая площадка сама добавляет колонки.
  Кто вызывает: pages/Live.svelte.
  От чего зависит: lib/liveGrid.js, lib/fmt.js, components/SpreadCell.svelte, LiveCoinModal.svelte.
  Настройки: нет.
-->
<script>
  import { venuePairs, coinRows, byBestSpread } from '../lib/liveGrid.js';
  import { clock } from '../lib/fmt.js';
  import SpreadCell from './SpreadCell.svelte';
  import LiveCoinModal from './LiveCoinModal.svelte';

  /** views — все SpreadView; match(asset) — фильтр по монете; onlyActive — «только выше порога». */
  let { views, match, onlyActive } = $props();

  /** Ключ монеты, открытой во всплывающем окне (null — окно закрыто). */
  let openKey = $state(null);

  const pairs = $derived(venuePairs(views));
  const all = $derived(coinRows(views));
  const rows = $derived(byBestSpread(all.filter((r) => match(r.asset) && (!onlyActive || r.active))));
  const legs = $derived(pairs.flatMap((p) => p.legs));
  const opened = $derived(openKey ? all.find((r) => r.key === openKey) : null);

  /** Причина «—» для направления, которого нет в данных ядра. */
  const missing = (r, leg) =>
    `${r.asset.ticker}: направления ${leg.buy} → ${leg.sell} нет в данных ядра — монета не прошла первичный фильтр для этой пары площадок (экран «Монеты»)`;
</script>

<div class="tscroll">
  <table class="compact">
    <thead>
      {#if pairs.length > 1}
        <tr>
          <th></th>
          {#each pairs as p (p.a + p.b)}<th colspan="6" class="group">{p.a} ↔ {p.b}</th>{/each}
          <th></th>
        </tr>
      {/if}
      <tr>
        <th rowspan="2">Монета</th>
        {#each legs as leg (leg.key)}<th colspan="3" class="group">{leg.buy} → {leg.sell}</th>{/each}
        <th rowspan="2">Обновлено</th>
      </tr>
      <tr>
        {#each legs as leg (leg.key)}
          <th class="num">спред %</th><th class="num">порог %</th><th class="group-end">состояние</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as r (r.key)}
        <tr class="click" onclick={() => (openKey = r.key)} title="Подробно: адрес, цены, причины">
          <td><strong>{r.asset.ticker}</strong></td>
          {#each legs as leg (leg.key)}<SpreadCell v={r.legs[leg.key]} missing={missing(r, leg)} />{/each}
          <td class="muted">{clock(r.at)}</td>
        </tr>
      {:else}
        <tr>
          <td colspan={legs.length * 3 + 2} class="muted">
            {#if views.length}Нет монет под фильтр.{:else}Монет нет: справочник пуст или ни одна монета ещё не прошла первичный фильтр (экран «Монеты»).{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

{#if opened}
  <LiveCoinModal row={opened} {legs} {missing} onclose={() => (openKey = null)} />
{/if}

<style>
  .group { text-align: center; border-left: 1px solid var(--line); }
  .group-end { border-right: 1px solid var(--line); }
</style>
