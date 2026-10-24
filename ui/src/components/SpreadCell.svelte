<!--
  Файл: ui/src/components/SpreadCell.svelte
  Что делает: одна компактная ячейка «Живого экрана» — одно направление монеты (купить на A → продать на B)
    в одну строку (Issue #33): спред % (3 знака), порог комиссий % (3 знака), короткое состояние со значком.
    Без цен, адресов и причин — они во всплывающем окне по щелчку на строке (LiveCoinModal.svelte).
    Нет направления в данных ядра — «—» с причиной в подсказке.
  Кто вызывает: components/LiveCoins.svelte.
  От чего зависит: lib/coreLabels.js (spreadShort), lib/fmt.js (fmtPct).
  Настройки: нет.
-->
<script>
  import { spreadShort } from '../lib/coreLabels.js';
  import { fmtPct } from '../lib/fmt.js';

  /**
   * v — SpreadView направления или undefined (ядро не прислало это направление);
   * missing — подпись причины для undefined.
   */
  let { v, missing } = $props();

  const st = $derived(v ? spreadShort(v.state) : null);
  const above = $derived(v?.spread_pct != null && v?.prices && Number(v.spread_pct) > Number(v.prices.fees_pct));
</script>

<td class="num spread" class:ok={above} title={v ? '' : missing}>{v ? fmtPct(v.spread_pct) : '—'}</td>
<td class="num muted">{v?.prices ? fmtPct(v.prices.fees_pct) : '—'}</td>
<td class="state {st?.cls ?? 'muted'}">{#if st}<span class="icon">{st.icon}</span> {st.text}{:else}—{/if}</td>

<style>
  .spread { font-weight: 600; }
  .state { border-right: 1px solid var(--line); }
  .icon { display: inline-block; width: 1em; text-align: center; }
</style>
