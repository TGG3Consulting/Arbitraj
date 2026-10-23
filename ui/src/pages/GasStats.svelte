<!--
  Файл: ui/src/pages/GasStats.svelte
  Что делает: экран «Статистика газа» (решение 36) — по каждой монете: попытки, успехи, отмены, газ впустую
    и всего (BNB), итог (USDT: прибыль минус убыток минус газ впустую), когда обновлено. Сортировка по столбцу.
    Числа Decimal приходят строками и показываются как есть (без пересчёта в браузере — без потери точности).
    Данные — GET /api/gas-stats; пока ядро не подключено — «нет данных» (503) с причиной.
    Живое сообщение gas_stats — перечитать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, lib/coinLabels.js, lib/labels.js, components/NoData.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import { assetLabel } from '../lib/coinLabels.js';
  import { showTime } from '../lib/labels.js';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const stats = remote('/api/gas-stats', ['gas_stats']);

  /** Столбцы: поле, заголовок, числовой ли (для выравнивания и сортировки). */
  const COLS = [
    ['attempts', 'Попытки', true],
    ['successes', 'Успехи', true],
    ['cancels', 'Отмены', true],
    ['gas_wasted_bnb', 'Газ впустую, BNB', true],
    ['gas_total_bnb', 'Газ всего, BNB', true],
    ['result_usdt', 'Итог, USDT', true],
  ];
  let sortBy = $state('');
  let desc = $state(true);

  /** Сравнение для сортировки: числа (в т.ч. строки Decimal) — по значению, только для порядка строк. */
  const cmp = (a, b) => Number(a) - Number(b);
  const rows = $derived.by(() => {
    const list = [...(stats.data ?? [])];
    if (sortBy) list.sort((x, y) => (desc ? -1 : 1) * cmp(x[sortBy], y[sortBy]));
    else list.sort((x, y) => x.asset.ticker.localeCompare(y.asset.ticker));
    return list;
  });

  /** Переключает сортировку по столбцу. Принимает: поле. */
  function sort(field) {
    if (sortBy === field) desc = !desc;
    else (sortBy = field), (desc = true);
  }
</script>

<h1>Статистика газа</h1>
<p class="muted">Сгоревший газ неудачных обменов в расчёт сделки не входит — он учитывается здесь, по монете.</p>
<NoData text={stats.noData} />
<ErrorText text={stats.error} />
{#if stats.loading}
  <p class="muted">Загружаю…</p>
{:else if stats.data}
  <table>
    <thead>
      <tr>
        <th>Монета</th>
        {#each COLS as [field, title] (field)}
          <th class="num"><button class="quiet" onclick={() => sort(field)}>{title}{sortBy === field ? (desc ? ' ↓' : ' ↑') : ''}</button></th>
        {/each}
        <th>Обновлено</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as s (`${s.asset.ticker}|${s.asset.bsc_contract}`)}
        <tr>
          <td>{assetLabel(s.asset)}</td>
          {#each COLS as [field] (field)}
            <td class="num" class:err={field === 'result_usdt' && String(s[field]).startsWith('-')}>{s[field]}</td>
          {/each}
          <td>{showTime(s.updated_at)}</td>
        </tr>
      {:else}
        <tr><td colspan={COLS.length + 2} class="muted">Обменов ещё не было.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}
