<!-- FermiSlotExecutor общий для выбранной Ethereum-площадки. Действующий строитель и слот проверяются при самой сделке. -->
<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {fermiNetwork,addFermi,registeredFermi} from '../lib/fermiDeploy.js';

  let {venue,wallet}=$props();
  const chain=$derived(fermiNetwork(venue,store.schema?.dex_profiles,store.values));
  const marketBindings=$derived({
    router:store.values['fermi.router|Venue|'+venue],engine:store.values['fermi.engine|Venue|'+venue],
    routerHash:store.values['fermi.router_runtime_hash|Venue|'+venue],
    runtimeKeccak:store.values['fermi.executor_runtime_hash|Venue|'+venue],
  });
  const load=()=>import('../assets/fermi-ethereum-create.json');
</script>

{#if chain}
  {#key [venue,wallet,chain,...Object.values(marketBindings)].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {marketBindings} {load} shared={true}
      source="fermi" version="1" kind="fermi" label="FermiSwap" setting="fermi.executor"
      addModule={addFermi} registeredOwnerModule={registeredFermi}/>
  {/key}
{/if}
