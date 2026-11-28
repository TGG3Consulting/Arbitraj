<!-- Карточка общего HanjiExecutor в строке кошелька. Профиль сети и адрес WETH берутся из сохранённых настроек. -->
<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {hanjiNetwork,addHanji,registeredHanji} from '../lib/hanjiDeploy.js';

  let {venue,wallet}=$props();
  const chain=$derived(hanjiNetwork(venue,store.schema?.dex_profiles,store.values));
  const wrapped=$derived(store.values['dex.native_wrapped|Venue|'+venue]);
  const load=()=>import('../assets/hanji-base-create.json');
</script>

{#if chain}
  {#key [venue,wallet,chain,wrapped].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {wrapped} {load} shared={true}
      source="hanji" version="1" kind="hanji" label="Hanji" setting="hanji.executor"
      addModule={addHanji} registeredOwnerModule={registeredHanji}/>
  {/key}
{/if}
