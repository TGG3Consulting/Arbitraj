<!-- Котировка Textile; кошелёк для защищённого исполнения привязывается к владельцу. -->
<script>
  import {store} from '../lib/settings.svelte.js';
  import SourcePrice from './SourcePrice.svelte';
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {addTextile,registeredTextile} from '../lib/textileDeploy.js';
  let {venue,trader,wallet}=$props();
  const chain=$derived(Number(store.values['dex.evm_chain_id|Venue|'+venue]));
</script>
<SourcePrice {venue} {trader} {wallet} source="textile" label="Textile FX"/>
{#if chain===8453}
  <SourceWalletModule {venue} wallet={trader} {chain} source="textile" setting="textile.contracts"
    label="Textile FX" version="v3" kind="textile_wallet" load={()=>import('../assets/textile-base-create.json')}
    addModule={addTextile} registeredOwnerModule={registeredTextile}/>
{/if}
