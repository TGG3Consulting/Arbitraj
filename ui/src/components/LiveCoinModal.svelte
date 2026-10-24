<!--
  Файл: ui/src/components/LiveCoinModal.svelte
  Что делает: всплывающее окно монеты «Живого экрана» (Issue #33): полный адрес контракта с кнопкой
    «Копировать»; по каждому направлению — точные цены покупки и продажи, объём сравнения, точный спред
    и порог комиссий, состояние фильтра шума и полная причина (каждая причина с новой строки), сколько
    обновлений подряд выше порога, с какого времени и время обновления. Рядом с точной ценой — коротко
    (6 значащих). Данные — те же SpreadView из GET /api/spreads и сообщений spread (без новых полей бэкенда); окно обновляется вживую.
  Кто вызывает: components/LiveCoins.svelte.
  От чего зависит: lib/coreLabels.js, lib/fmt.js, lib/labels.js, components/Modal.svelte, CopyAddr.svelte.
  Настройки: нет.
-->
<script>
  import { spreadState, spreadShort, spreadReason } from '../lib/coreLabels.js';
  import { reasonLines, fmtPrice } from '../lib/fmt.js';
  import { showTime } from '../lib/labels.js';
  import Modal from './Modal.svelte';
  import CopyAddr from './CopyAddr.svelte';

  /**
   * row — строка монеты (liveGrid.coinRows); legs — направления колонок [{buy, sell, key}];
   * missing(row, leg) — причина, если направления нет в данных; onclose() — закрыть окно.
   */
  let { row, legs, missing, onclose } = $props();
</script>

<Modal title={row.asset.ticker} {onclose}>
  <h4>Адрес контракта в BSC</h4>
  <CopyAddr addr={row.asset.bsc_contract} />

  {#each legs as leg (leg.key)}
    {@const v = row.legs[leg.key]}
    <section class="card">
      <h3>Купить на {leg.buy} → продать на {leg.sell}</h3>
      {#if !v}
        <p class="muted">{missing(row, leg)}</p>
      {:else}
        {@const st = spreadShort(v.state)}
        {@const why = reasonLines(spreadReason(v.state))}
        <dl>
          <dt>Состояние</dt>
          <dd class={st.cls}>{st.icon} {why.length ? st.text : spreadState(v.state).text}</dd>
          {#if why.length}
            <dt>Причина</dt>
            <dd><ul class="why">{#each why as line, i (i)}<li>{line}</li>{/each}</ul></dd>
          {/if}
          <dt>Спред</dt>
          <dd><code>{v.spread_pct ?? '—'}</code>{v.spread_pct != null ? ' %' : ''}</dd>
          <dt>Порог комиссий</dt>
          <dd><code>{v.prices?.fees_pct ?? '—'}</code>{v.prices ? ' %' : ''}</dd>
          <dt>Цена покупки на {leg.buy}</dt>
          <dd><code>{v.prices?.buy ?? '—'}</code>{#if v.prices} <span class="muted">≈ {fmtPrice(v.prices.buy)}</span>{/if}</dd>
          <dt>Цена продажи на {leg.sell}</dt>
          <dd><code>{v.prices?.sell ?? '—'}</code>{#if v.prices} <span class="muted">≈ {fmtPrice(v.prices.sell)}</span>{/if}</dd>
          <dt>Объём сравнения</dt>
          <dd><code>{v.prices?.qty ?? '—'}</code>{v.prices ? ` ${row.asset.ticker}` : ''}</dd>
          <dt>Выше порога</dt>
          <dd>
            {#if v.above_since}с {showTime(v.above_since)}, обновлений подряд: {v.updates}{:else}нет{/if}
          </dd>
          <dt>Обновлено</dt>
          <dd>{showTime(v.at)}</dd>
        </dl>
      {/if}
    </section>
  {/each}
</Modal>

<style>
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 16px; margin: 0; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .why { margin: 0; padding-left: 18px; }
  .why li { margin: 2px 0; }
  h3 { margin-top: 0; }
</style>
