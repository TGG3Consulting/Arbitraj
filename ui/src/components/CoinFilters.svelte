<!--
  Файл: ui/src/components/CoinFilters.svelte
  Что делает: строка фильтров экрана «Монеты» (Issue #33): поиск по тикеру или адресу; итог отбора
    (прошла или причина отсева); площадка; есть/нет пулов; вид пула; налоги (проверено / не проверено / ошибка);
    «только чёрный список». Списки площадок и видов пулов — только из данных (lib/coinFilter.js options).
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/coinFilter.js (RESULTS), lib/labels.js (poolKindLabel).
  Настройки: нет.
-->
<script>
  import { RESULTS } from '../lib/coinFilter.js';
  import { poolKindLabel } from '../lib/labels.js';

  /** f — объект фильтров (bindable): {text, result, venue, pools, kind, tax, onlyBlocked}; opts — {venues, kinds}. */
  let { f = $bindable(), opts } = $props();
</script>

<div class="filters">
  <input bind:value={f.text} placeholder="тикер или адрес" />
  <select bind:value={f.result} aria-label="итог отбора">
    <option value="">итог отбора: все</option>
    {#each RESULTS as [k, label] (k)}<option value={k}>{label}</option>{/each}
  </select>
  <select bind:value={f.venue} aria-label="площадка">
    <option value="">площадка: все</option>
    {#each opts.venues as v (v)}<option value={v}>{v}</option>{/each}
  </select>
  <select bind:value={f.pools} aria-label="пулы">
    <option value="">пулы: все</option>
    <option value="yes">есть пулы</option>
    <option value="no">нет пулов</option>
  </select>
  <select bind:value={f.kind} aria-label="вид пула">
    <option value="">вид пула: все</option>
    {#each opts.kinds as k (k)}<option value={k}>{poolKindLabel(k)}</option>{/each}
  </select>
  <select bind:value={f.tax} aria-label="налоги">
    <option value="">налоги: все</option>
    <option value="known">проверено</option>
    <option value="not_checked">не проверено</option>
    <option value="failed">ошибка проверки</option>
  </select>
  <label class="inline"><input type="checkbox" bind:checked={f.onlyBlocked} /> только чёрный список</label>
</div>
