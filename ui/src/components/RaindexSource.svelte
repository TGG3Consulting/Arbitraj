<!-- Карточки размещения подтверждённых книг Raindex для выбранного кошелька Base. Строка кошелька передаёт venue и адрес. -->
<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {baseNetwork} from '../lib/baseNetwork.js';
  import {store} from '../lib/settings.svelte.js';
  import {addRaindex,registeredRaindex} from '../lib/raindexDeploy.js';
  let {venue,wallet}=$props();
  const chain=$derived(baseNetwork(venue,store.schema?.dex_profiles,store.values));
  const rows=$derived(store.values['raindex.books|Venue|'+venue]);
  const load=()=>import('../assets/raindex-base-create.json');
  const artifact=$derived(chain && load());
  // Адрес для отображения. Перед каждой операцией loadRaindex отдельно проверяет всю строку.
  function displayed(book){
    return Array.isArray(rows) ? rows.map(row=>String(row).trim().split(/\s+/))
      .find(parts=>parts[2]?.toLowerCase()===book)?.[4] : null;
  }
</script>

{#if artifact}
  {#await artifact then card}
    {#each card.default.books as b ([venue,wallet,b.book].join(':'))}
      {#if b.settlementVerified===false}
        <p>Raindex V{b.version}. Архивная проверка выплаты через модуль по этой книге ещё не прошла. Перед торговлей бот проверяет списание и зачисление для каждого ордера.</p>
      {/if}
      <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} book={b.book} bookModule={displayed(b.book)}
        source={'raindex-'+b.book} version={b.version} kind="raindex" label={'Raindex V'+b.version} setting="raindex.books"
        addModule={addRaindex} registeredOwnerModule={registeredRaindex}/>
    {/each}
  {:catch error}
    <p class="err" role="alert">Не удалось загрузить карточки Raindex: {error.message}</p>
  {/await}
{/if}
