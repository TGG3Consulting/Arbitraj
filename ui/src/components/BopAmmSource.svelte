<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {ethereumNetwork} from '../lib/ethereumNetwork.js';
  import {addBop,registeredBop} from '../lib/bopammDeploy.js';
  let {venue,wallet}=$props();
  const chain=$derived(ethereumNetwork(venue,store.schema?.dex_profiles,store.values));
  const load=()=>import('../assets/bopamm-ethereum-create.json');
</script>

{#if chain}
  {#key [venue,wallet,chain].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} roleRows={true}
      source="bopamm" version="1" kind="bopamm" label="bopAMM" setting="bopamm.roles"
      addModule={addBop} registeredOwnerModule={registeredBop}/>
  {/key}
{/if}
