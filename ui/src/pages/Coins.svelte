<!--
  Файл: ui/src/pages/Coins.svelte
  Что делает: экран «Монеты» во всю ширину окна (Issue #33): ручной чёрный список; компактный список монет
    справочника (GET /api/coins) — тикер, адрес коротко, итог отбора, налоги коротко, число пулов;
    фильтры сверху (поиск по тикеру/адресу, итог отбора, площадка, есть/нет пулов, вид пула, налоги,
    только чёрный список); щелчок по монете — всплывающее окно с полной информацией (налоги, пулы, крючки,
    причины по площадкам, «Перепроверить»). Окно обновляется вместе с живыми данными.
    Пока ядро не отдаёт справочник — честное «нет данных» с причиной бэкенда (503); ручной список работает всегда.
    Живые сообщения coin_check, pool_check, coin_block — перечитать.
    Решение 116: таблица — по страницам (поле «Монет на странице», запоминается в браузере; без значения таблица
    не рисуется и просит его задать). С MEXC в справочнике >11 тыс. монет — все строки разом тормозили экран.
    Смена фильтров — снова первая страница.
    Решение 200: блок «Один адрес — разные тикеры» (TickerAlias) — кнопка «Считать одной монетой» (coin.ticker_alias).
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, lib/coinFilter.js, components/CoinFilters.svelte, CoinTable.svelte,
    CoinModal.svelte, ManualBlacklist.svelte, TickerAlias.svelte, NoData.svelte, ErrorText.svelte, PageSize.svelte.
  Настройки: coin.blocked, coin.blocked_note (через ManualBlacklist); пороги tax.*, hooks.* — экран «Настройки».
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import { coinRowKey, matches, options } from '../lib/coinFilter.js';
  import CoinFilters from '../components/CoinFilters.svelte';
  import CoinTable from '../components/CoinTable.svelte';
  import CoinModal from '../components/CoinModal.svelte';
  import ManualBlacklist from '../components/ManualBlacklist.svelte';
  import TickerAlias from '../components/TickerAlias.svelte';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import PageSize from '../components/PageSize.svelte';

  const coins = remote('/api/coins', ['coin_check', 'pool_check', 'coin_block']);
  let f = $state({ text: '', result: '', venue: '', pools: '', kind: '', tax: '', onlyBlocked: false });
  /** Ключ монеты в окне (тикер|адрес); null — окно закрыто. */
  let openKey = $state(null);

  const key = coinRowKey;
  const all = $derived(coins.data ?? []);
  const shown = $derived(all.filter((c) => matches(c, f)));
  const opts = $derived(options(all));
  const blockedCount = $derived(all.filter((c) => c.block).length);
  const passedCount = $derived(all.filter((c) => (c.directions ?? []).length).length);
  const opened = $derived(openKey ? all.find((c) => key(c) === openKey) : null);

  /** Монет на странице (null — не задано) и номер страницы с 1. */
  let size = $state(null);
  let page = $state(1);
  const pages = $derived(size ? Math.max(1, Math.ceil(shown.length / size)) : 1);
  const pageRows = $derived(size ? shown.slice((page - 1) * size, page * size) : []);
  // Смена фильтров — с первой страницы; страница за пределами (список сократился) — последняя.
  $effect(() => {
    JSON.stringify(f);
    page = 1;
  });
  $effect(() => {
    if (page > pages) page = pages;
  });
</script>

<h1>Монеты</h1>
<ManualBlacklist />
<TickerAlias />

<h2>Справочник, налоги и пулы</h2>
<NoData text={coins.noData} />
<ErrorText text={coins.error} />
{#if coins.loading}
  <p class="muted">Загружаю…</p>
{:else if coins.data}
  <p class="muted">
    Монет: {all.length}, прошли отбор: {passedCount}, в чёрном списке: {blockedCount}; показано: {shown.length}.
  </p>
  <CoinFilters bind:f {opts} />
  <div class="row">
    <PageSize key="coins.page" bind:value={size} label="Монет на странице:" />
    {#if size}
      <button disabled={page <= 1} onclick={() => (page -= 1)}>‹</button>
      <span>страница {page} из {pages}</span>
      <button disabled={page >= pages} onclick={() => (page += 1)}>›</button>
    {/if}
  </div>
  {#if size}
    <CoinTable coins={pageRows} onopen={(c) => (openKey = key(c))} />
  {:else}
    <p class="muted">Задайте «Монет на странице», чтобы показать таблицу.</p>
  {/if}
{/if}

{#if opened}
  <CoinModal coin={opened} onclose={() => (openKey = null)} />
{/if}
