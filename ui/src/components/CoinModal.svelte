<!--
  Файл: ui/src/components/CoinModal.svelte
  Что делает: всплывающее окно монеты на экране «Монеты» (Issue #33): полный адрес с кнопкой «Копировать»,
    итог отбора (каждая причина с новой строки) и полная карточка монеты — площадки, налоги, чёрный список,
    причины по площадкам, пулы с крючками и проверками, «Перепроверить» (CoinRow.svelte); пулы монеты из справочника
    индексатора с итогом бота (CoinDirectory.svelte, решение 58).
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/coinFilter.js, components/Modal.svelte, CopyAddr.svelte, CoinRow.svelte, CoinDirectory.svelte.
  Настройки: нет.
-->
<script>
  import { uniq } from '../lib/uniq.js';
  import { coinTitle, reasons, resultLabel } from '../lib/coinFilter.js';
  import Modal from './Modal.svelte';
  import CopyAddr from './CopyAddr.svelte';
  import CoinRow from './CoinRow.svelte';
  import CoinDirectory from './CoinDirectory.svelte';

  /** coin — элемент GET /api/coins (свежий: экран передаёт его из живых данных); onclose() — закрыть. */
  let { coin, onclose } = $props();
  const rs = $derived(reasons(coin));
</script>

<Modal title={coinTitle(coin)} {onclose}>
  <h4>Адрес контракта в BSC</h4>
  <CopyAddr addr={coin.asset.bsc_contract} />
  <h4>Итог отбора</h4>
  <ul class="res">
    {#each uniq(rs) as k (k)}<li class={k === 'passed' ? 'ok' : 'err'}>{resultLabel(k)}</li>{/each}
  </ul>
  {#if rs[0] === 'passed'}
    <p class="muted">Направлений: {coin.directions.length} — {coin.directions.map((d) => `${d.buy_on} → ${d.sell_on}`).join(', ')}</p>
  {/if}
  <CoinRow {coin} />
  <CoinDirectory addr={coin.asset.bsc_contract} />
</Modal>

<style>
  .res { margin: 0; padding-left: 20px; }
</style>
