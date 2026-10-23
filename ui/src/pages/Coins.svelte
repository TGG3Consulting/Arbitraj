<!--
  Файл: ui/src/pages/Coins.svelte
  Что делает: экран «Монеты» — налоги покупки/продажи/перевода и состояние проверки, пулы с крючками
    и группами A/B/C, чёрный список с причинами (GET /api/coins), «Перепроверить» по монете, ручной чёрный
    список (добавить/убрать). Фильтр по тикеру и «только в чёрном списке».
    Пока ядро не отдаёт справочник — честное «нет данных» с причиной бэкенда (503); ручной список работает всегда.
    Живые сообщения coin_check, pool_check, coin_block — перечитать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, components/CoinRow.svelte, ManualBlacklist.svelte, NoData.svelte, ErrorText.svelte.
  Настройки: coin.blocked, coin.blocked_note (через ManualBlacklist); пороги tax.*, hooks.* — экран «Настройки».
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import CoinRow from '../components/CoinRow.svelte';
  import ManualBlacklist from '../components/ManualBlacklist.svelte';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const coins = remote('/api/coins', ['coin_check', 'pool_check', 'coin_block']);
  let filter = $state('');
  let onlyBlocked = $state(false);

  const shown = $derived(
    (coins.data ?? []).filter(
      (c) =>
        c.asset.ticker.toLowerCase().includes(filter.trim().toLowerCase()) && (!onlyBlocked || c.block),
    ),
  );
  const blockedCount = $derived((coins.data ?? []).filter((c) => c.block).length);
</script>

<h1>Монеты</h1>
<ManualBlacklist />

<h2>Справочник, налоги и пулы</h2>
<NoData text={coins.noData} />
<ErrorText text={coins.error} />
{#if coins.loading}
  <p class="muted">Загружаю…</p>
{:else if coins.data}
  <p class="muted">Монет: {coins.data.length}, в чёрном списке: {blockedCount}.</p>
  <div class="row">
    <input bind:value={filter} placeholder="тикер" />
    <label class="inline"><input type="checkbox" bind:checked={onlyBlocked} /> только в чёрном списке</label>
  </div>
  {#each shown as coin (`${coin.asset.ticker}|${coin.asset.bsc_contract}`)}
    <CoinRow {coin} />
  {:else}
    <p class="muted">Ничего не найдено.</p>
  {/each}
{/if}
