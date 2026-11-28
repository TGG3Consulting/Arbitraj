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
  let chosen=$state('');
  // Адрес для отображения. Перед каждой операцией loadRaindex отдельно проверяет всю строку.
  function displayed(book){
    return Array.isArray(rows) ? rows.map(row=>String(row).trim().split(/\s+/))
      .find(parts=>parts[2]?.toLowerCase()===book)?.[4] : null;
  }
</script>

{#if artifact}
  {#await artifact then card}
    {@const b=card.default.books.find(book=>book.book===chosen) ?? card.default.books[0]}
    <label>Книга Raindex в Base
      <select value={b.book} onchange={e=>chosen=e.currentTarget.value}>
        {#each card.default.books as book (book.book)}
          <option value={book.book}>V{book.version} · {book.book}{displayed(book.book) ? ' · настроена' : ''}</option>
        {/each}
      </select>
    </label>
    {#key [venue,wallet,b.book].join(':')}
      <p>Блок размещения книги {b.startBlock}.</p>
      {#if b.sale?.paid}
        <p>Продажа {b.sale.asset} за USDC подтверждена на двух размерах напрямую и через модуль.</p>
      {:else if b.sale}
        <p>Продажа {b.sale.asset} за USDC по проверенным заявкам откатилась на обоих размерах напрямую и через модуль. Ответ книги: <code>{b.sale.reason}</code>.</p>
      {:else if b.settlementVerified===true}
        <p>В архивной симуляции подтверждены списание и выплата через модуль этой книги.</p>
      {:else}
        <p>Архивного измерения выплаты через модуль этой книги пока нет.</p>
      {/if}
      {#if b.projectFundingVerified===true}
        <p>Покупка с USDC проверена на средствах исходного кошелька в симуляции.</p>
      {:else if b.projectFundingVerified===false}
        <p>Покупка с USDC на исходном кошельке откатилась в архивной симуляции. У держателя токена подтверждена продажа за USDC.</p>
      {:else}
        <p>Покупка с USDC на исходном кошельке ещё не измерена.</p>
      {/if}
      <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} book={b.book} bookModule={displayed(b.book)}
        source={'raindex-'+b.book} version={b.version} kind="raindex" label={'Raindex V'+b.version} setting="raindex.books"
        addModule={addRaindex} registeredOwnerModule={registeredRaindex}/>
    {/key}
  {:catch error}
    <p class="err" role="alert">Не удалось загрузить карточки Raindex: {error.message}</p>
  {/await}
{/if}
