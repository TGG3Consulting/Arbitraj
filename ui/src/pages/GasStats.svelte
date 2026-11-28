<!--
  Файл: ui/src/pages/GasStats.svelte
  Что делает: экран «Статистика газа» (решение 36) — по каждой монете: попытки, успехи, отмены, газ впустую
    и всего (в монете газа сети монеты — поле gas ответа, dex.gas_ticker: BNB у BSC, ETH у Base/Ethereum), итог (USDT: прибыль минус убыток минус газ впустую), когда обновлено. Сортировка по столбцу.
    Числа Decimal приходят строками и показываются как есть (без пересчёта в браузере — без потери точности).
    Данные — GET /api/gas-stats; пока ядро не подключено — «нет данных» (503) с причиной.
    Живое сообщение gas_stats — перечитать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, lib/coinLabels.js, lib/coinNames.svelte.js, lib/labels.js, components/NoData.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import { assetLabel } from '../lib/coinLabels.js';
  import { assetId } from '../lib/coinNames.svelte.js';
  import { showTime } from '../lib/labels.js';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const stats = remote('/api/gas-stats', ['gas_stats']);

  /** Поля газа: значение — в монете газа сети монеты (поле gas строки — dex.gas_ticker, аудит 28.11.2026). */
  const GAS = ['gas_wasted_bnb', 'gas_total_bnb'];
  /** Один тикер газа у всех строк — он в заголовке (у BSC — «BNB», как раньше); разные или не задан — тикер у числа. */
  const unit = $derived.by(() => {
    const set = new Set((stats.data ?? []).map((s) => s.gas ?? null));
    return set.size === 1 ? [...set][0] : null;
  });
  /** Столбцы: поле, заголовок, числовой ли (для выравнивания и сортировки). */
  const COLS = $derived([
    ['attempts', 'Попытки', true],
    ['successes', 'Успехи', true],
    ['cancels', 'Отмены', true],
    ['gas_wasted_bnb', unit ? `Газ впустую, ${unit}` : 'Газ впустую', true],
    ['gas_total_bnb', unit ? `Газ всего, ${unit}` : 'Газ всего', true],
    ['result_usdt', 'Итог, USDT', true],
  ]);
  /** Число ячейки: у поля газа без общего тикера — с тикером строки (не задан — «монета газа»). */
  const cell = (s, field) => (GAS.includes(field) && !unit ? `${s[field]} ${s.gas ?? 'монета газа'}` : s[field]);
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
      {#each rows as s, si (`${assetId(s.asset)}|${si}`)}
        <tr>
          <td>{assetLabel(s.asset)}</td>
          {#each COLS as [field] (field)}
            <td class="num" class:err={field === 'result_usdt' && String(s[field]).startsWith('-')}>{cell(s, field)}</td>
          {/each}
          <td>{showTime(s.updated_at)}</td>
        </tr>
      {:else}
        <tr><td colspan={COLS.length + 2} class="muted">Обменов ещё не было.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}
