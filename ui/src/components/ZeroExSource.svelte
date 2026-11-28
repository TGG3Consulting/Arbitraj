<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {nativeRfqNetwork} from '../lib/nativeRfqDeploy.js';
  import {addZeroEx,registeredZeroEx} from '../lib/zeroexDeploy.js';
  let {venue,wallet}=$props();
  const chain=$derived(nativeRfqNetwork(venue,store.schema?.dex_profiles,store.values));
  const load=()=>import('../assets/zeroex-ethereum-create.json');
</script>
{#if chain===1}
  {#key [venue,wallet,chain].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} source="zeroex" kind="zeroex" version="1"
      label="0x Ethereum" setting="zeroex.wallet" addModule={addZeroEx} registeredOwnerModule={registeredZeroEx}/>
  {/key}
{/if}
