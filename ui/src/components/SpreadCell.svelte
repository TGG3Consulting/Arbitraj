<!--
  Файл: ui/src/components/SpreadCell.svelte
  Что делает: одна ячейка «Живого экрана» — одно направление монеты (купить на A → продать на B):
    спред %, порог комиссий %, состояние фильтра шума, цены покупки и продажи.
    Нет спреда (state NoPrice / NotConfigured / нет направления в данных) — «—», причина во всплывающей подсказке.
  Кто вызывает: components/LiveCoins.svelte.
  От чего зависит: lib/coreLabels.js (spreadState).
  Настройки: нет.
-->
<script>
  import { spreadState } from '../lib/coreLabels.js';

  /**
   * v — SpreadView направления или undefined (ядро не прислало это направление);
   * missing — подпись причины для undefined.
   */
  let { v, missing } = $props();

  const st = $derived(v ? spreadState(v.state) : null);
  const above = $derived(v?.spread_pct != null && v?.prices && Number(v.spread_pct) > Number(v.prices.fees_pct));
  /** Подсказка: состояние с причиной и число обновлений подряд выше порога. */
  const tip = $derived(v ? `${st.text}${v.updates ? ` · ${v.updates} обн. подряд выше порога` : ''}` : missing);
</script>

<td class="leg" title={tip}>
  {#if !v}
    <span class="muted">—</span>
  {:else}
    <div class="num big" class:ok={above}>{v.spread_pct ?? '—'}{v.spread_pct != null ? ' %' : ''}</div>
    <div class="small">порог {v.prices?.fees_pct ?? '—'}{v.prices ? ' %' : ''}</div>
    <div class="small {st.cls}">{st.text}</div>
    {#if v.prices}
      <div class="small muted">{v.prices.buy} → {v.prices.sell}</div>
    {/if}
  {/if}
</td>

<style>
  .leg { vertical-align: top; min-width: 150px; }
  .big { font-size: 1.1em; font-weight: 600; text-align: left; }
  .small { font-size: 0.85em; }
</style>
