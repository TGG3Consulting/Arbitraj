<!--
  Файл: ui/src/components/LiveCoins.svelte
  Что делает: компактная таблица «Живого экрана» по монетам, вид «Б» (решение 106): одна строка — одна монета,
    прошедшая отбор; в строке — только ЛУЧШЕЕ направление (купить на A → продать на B; среди выше порога — с наибольшим
    спредом): спред %, порог %, короткое состояние; сколько у монеты направлений и сколько выше порога; время.
    Щелчок по строке раскрывает под ней все направления монеты (по убыванию спреда), кнопка «подробно» — всплывающее
    окно с ценами и полными причинами (LiveCoinModal.svelte), оно обновляется вживую. Ширина таблицы не растёт
    с числом площадок: новая биржа добавляет направления в раскрытие, а не колонки. Имён площадок в коде нет.
  Кто вызывает: pages/Live.svelte.
  От чего зависит: lib/liveGrid.js, lib/fmt.js, components/SpreadCell.svelte, LiveCoinModal.svelte.
  Настройки: нет.
-->
<script>
  import { coinRows, byBestSpread, rowLegs } from '../lib/liveGrid.js';
  import { clock } from '../lib/fmt.js';
  import SpreadCell from './SpreadCell.svelte';
  import LiveCoinModal from './LiveCoinModal.svelte';

  /** views — все SpreadView; match(asset) — фильтр по монете; onlyActive — «только выше порога». */
  let { views, match, onlyActive } = $props();

  /** Ключ монеты во всплывающем окне (null — закрыто) и раскрытые строки. */
  let openKey = $state(null);
  let expanded = $state({});

  const all = $derived(coinRows(views));
  const rows = $derived(byBestSpread(all.filter((r) => match(r.asset) && (!onlyActive || r.active))));
  const opened = $derived(openKey ? all.find((r) => r.key === openKey) : null);

  /** Причина «—» для направления, которого нет в данных ядра. */
  const missing = (r, leg) =>
    `${r.asset.ticker}: направления ${leg.buy} → ${leg.sell} нет в данных ядра — монета не прошла первичный фильтр для этой пары площадок (экран «Монеты»)`;

  /** Раскрыть или свернуть строку монеты. */
  const toggle = (key) => (expanded[key] = !expanded[key]);
  /** Направление по ключу «A>B». */
  const legOf = (r, key) => ({ buy: r.legs[key].direction.buy_on, sell: r.legs[key].direction.sell_on, key });
</script>

<div class="tscroll">
  <table class="compact">
    <thead>
      <tr>
        <th></th>
        <th>Монета</th>
        <th>Лучшее направление</th>
        <th class="num">спред %</th><th class="num">порог %</th><th>состояние</th>
        <th class="num">направлений</th>
        <th>Обновлено</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r (r.key)}
        {@const n = Object.keys(r.legs).length}
        <tr class="click" onclick={() => toggle(r.key)} title="Все направления монеты">
          <td class="muted">{expanded[r.key] ? '▾' : '▸'}</td>
          <td><strong>{r.asset.ticker}</strong></td>
          {#if r.bestKey}
            {@const leg = legOf(r, r.bestKey)}
            <td>{leg.buy} → {leg.sell}</td>
            <SpreadCell v={r.legs[r.bestKey]} missing={missing(r, leg)} />
          {:else}
            <td class="muted">—</td><td></td><td></td><td></td>
          {/if}
          <td class="num">{n}{#if r.activeCount} <span class="ok">/ {r.activeCount} выше</span>{/if}</td>
          <td class="muted">{clock(r.at)}</td>
          <td><button class="quiet" onclick={(e) => { e.stopPropagation(); openKey = r.key; }}>подробно</button></td>
        </tr>
        {#if expanded[r.key]}
          {#each rowLegs(r) as leg (leg.key)}
            <tr class="sub">
              <td></td><td></td>
              <td>{leg.buy} → {leg.sell}</td>
              <SpreadCell v={r.legs[leg.key]} missing={missing(r, leg)} />
              <td></td><td class="muted">{clock(r.legs[leg.key].at)}</td><td></td>
            </tr>
          {/each}
        {/if}
      {:else}
        <tr>
          <td colspan="9" class="muted">
            {#if views.length}Нет монет под фильтр.{:else}Монет нет: справочник пуст или ни одна монета ещё не прошла первичный фильтр (экран «Монеты»).{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

{#if opened}
  <LiveCoinModal row={opened} legs={rowLegs(opened)} {missing} onclose={() => (openKey = null)} />
{/if}

<style>
  .sub td { background: var(--bg-soft, transparent); font-size: 0.95em; }
</style>
