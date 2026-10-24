<!--
  Файл: ui/src/pages/Coins.svelte
  Что делает: экран «Монеты» во всю ширину окна (Issue #33): ручной чёрный список; компактный список монет
    справочника (GET /api/coins) — тикер, адрес коротко, итог отбора, налоги коротко, число пулов;
    фильтры сверху (поиск по тикеру/адресу, итог отбора, площадка, есть/нет пулов, вид пула, налоги,
    только чёрный список); щелчок по монете — всплывающее окно с полной информацией (налоги, пулы, крючки,
    причины по площадкам, «Перепроверить»). Окно обновляется вместе с живыми данными.
    Пока ядро не отдаёт справочник — честное «нет данных» с причиной бэкенда (503); ручной список работает всегда.
    Живые сообщения coin_check, pool_check, coin_block — перечитать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, lib/coinFilter.js, components/CoinFilters.svelte, CoinTable.svelte,
    CoinModal.svelte, ManualBlacklist.svelte, NoData.svelte, ErrorText.svelte.
  Настройки: coin.blocked, coin.blocked_note (через ManualBlacklist); пороги tax.*, hooks.* — экран «Настройки».
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import { matches, options } from '../lib/coinFilter.js';
  import CoinFilters from '../components/CoinFilters.svelte';
  import CoinTable from '../components/CoinTable.svelte';
  import CoinModal from '../components/CoinModal.svelte';
  import ManualBlacklist from '../components/ManualBlacklist.svelte';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const coins = remote('/api/coins', ['coin_check', 'pool_check', 'coin_block']);
  let f = $state({ text: '', result: '', venue: '', pools: '', kind: '', tax: '', onlyBlocked: false });
  /** Ключ монеты в окне (тикер|адрес); null — окно закрыто. */
  let openKey = $state(null);

  const key = (c) => `${c.asset.ticker}|${c.asset.bsc_contract}`;
  const all = $derived(coins.data ?? []);
  const shown = $derived(all.filter((c) => matches(c, f)));
  const opts = $derived(options(all));
  const blockedCount = $derived(all.filter((c) => c.block).length);
  const passedCount = $derived(all.filter((c) => (c.directions ?? []).length).length);
  const opened = $derived(openKey ? all.find((c) => key(c) === openKey) : null);
</script>

<h1>Монеты</h1>
<ManualBlacklist />

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
  <CoinTable coins={shown} onopen={(c) => (openKey = key(c))} />
{/if}

{#if opened}
  <CoinModal coin={opened} onclose={() => (openKey = null)} />
{/if}
