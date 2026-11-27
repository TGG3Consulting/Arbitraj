<script>
  import {store} from '../lib/settings.svelte.js';
  import SourcePrice from './SourcePrice.svelte';
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {addModule,registeredOwnerModule} from '../lib/airswapDeploy.js';
  let {venue,trader,wallet}=$props();
  const chain=$derived(Number(store.values['dex.evm_chain_id|Venue|'+venue]));
  const rows=$derived(store.values['airswap.contracts|Venue|'+venue] ?? []);
  const loads={
    '1:v4':()=>import('../assets/airswap-ethereum-v4-create.json'),
    '1:v5':()=>import('../assets/airswap-ethereum-v5-create.json'),
    '8453:v5':()=>import('../assets/airswap-base-v5-create.json')
  };
  const versions=$derived(['v4','v5'].filter(v=>loads[chain+':'+v] && rows.some(line=>{
    const [n,version]=line.trim().toLowerCase().split(/\s+/);
    return Number(n)===chain && version===v;
  })));
</script>
{#if versions.length}
  <SourcePrice {venue} trader={trader} {wallet} source="airswap" label="AirSwap"/>
  {#each versions as version ([venue,trader,chain,version].join(':'))}
    <SourceWalletModule source="airswap" setting="airswap.contracts" kind={'airswap_'+version} label={'AirSwap '+version.toUpperCase()} {addModule} {registeredOwnerModule} {venue} wallet={trader} {chain} {version} load={loads[chain+':'+version]}/>
  {/each}
{/if}
