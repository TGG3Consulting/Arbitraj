<!-- Один общий расчётный модуль для источника Native V6 выбранной сети. -->
<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {addNative,registeredNative,nativeRfqNetwork} from '../lib/nativeRfqDeploy.js';

  let {venue,wallet}=$props();
  const chain=$derived(nativeRfqNetwork(venue,store.schema?.dex_profiles,store.values));
  const load=()=>chain===1 ? import('../assets/native-rfq-ethereum-create.json') : import('../assets/native-rfq-base-create.json');
</script>

{#if chain}
  {#key [venue,wallet,chain].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} roleRows={true}
      source="native-rfq" version="6" kind="native_rfq" label="Native RFQ V6" setting="native_rfq.roles"
      addModule={addNative} registeredOwnerModule={registeredNative}/>
  {/key}
{/if}
