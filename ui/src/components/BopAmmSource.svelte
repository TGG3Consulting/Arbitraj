<script>
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {store} from '../lib/settings.svelte.js';
  import {ethereumNetwork} from '../lib/ethereumNetwork.js';
  import {addBop,registeredBop} from '../lib/bopammDeploy.js';
  let {venue,wallet}=$props();
  const chain=$derived(ethereumNetwork(venue,store.schema?.dex_profiles,store.values));
  const marketBindings=$derived({cashRows:store.values['bopamm.cash_path|Venue|'+venue]});
  const cash=()=>import('../assets/bopamm-cash-ethereum-create.json');
  const load=()=>import('../assets/bopamm-ethereum-create.json');
</script>

{#if chain}
  {#key [venue,wallet,chain].join(':')}
    <SourceWalletModule {venue} {wallet} {chain} {load} shared={true} roleRows={true}
      source="bopamm" version="1" kind="bopamm" label="bopAMM" setting="bopamm.roles"
      addModule={addBop} registeredOwnerModule={registeredBop}/>
    <SourceWalletModule {venue} {wallet} {chain} load={cash} {marketBindings} roleName="cash_module" shared={true} roleRows={true}
      source="bopamm" version="2" kind="bopamm" label="bopAMM и Uniswap V3 через USDC" setting="bopamm.roles"
      addModule={addBop} registeredOwnerModule={registeredBop}/>
  {/key}
{/if}
