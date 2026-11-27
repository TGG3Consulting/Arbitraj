<script>
  import {DEXALOT_CHAIN_ID} from '../lib/dexalotNetwork.js';
  import {store} from '../lib/settings.svelte.js';
  import SourcePrice from './SourcePrice.svelte';
  import SourceWalletModule from './SourceWalletModule.svelte';
  import {addDexalot} from '../lib/dexalotDeploy.js';
  import {recoverDexalot,registeredDexalotVersions} from '../lib/dexalotVersions.js';
  let {venue,trader,wallet}=$props();
  const chain=$derived(Number(store.values['dex.evm_chain_id|Venue|'+venue]));
  const url=$derived(store.values['dexalot.base_url|Venue|'+venue]);
  const rows=$derived(store.values['dexalot.roles|Venue|'+venue] ?? []);
  const rolesReady=$derived(rows.some(r=>/^router\s/i.test(r)) && rows.some(r=>/^maker\s/i.test(r)));
</script>
{#if chain===DEXALOT_CHAIN_ID && url}
  <SourcePrice {venue} {trader} {wallet} source="dexalot" label="Dexalot"/>
  {#if rolesReady}
    <SourceWalletModule {venue} wallet={trader} {chain} source="dexalot" setting="dexalot.roles" kind="dexalot_rfq"
      label="Dexalot" version="rfq" load={()=>import('../assets/dexalot-base-create.json')} addModule={addDexalot} recoverPlan={recoverDexalot} registeredOwnerModule={registeredDexalotVersions}/>
  {/if}
{/if}
