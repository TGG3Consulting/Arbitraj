<!-- Ручная котировка для адреса выбранного кошелька. Сумма остаётся десятичной строкой. -->
<script>
  import { post, seg } from '../lib/api.js';
  import {store} from '../lib/settings.svelte.js';
  import {sameHashflowWallet} from '../lib/hashflowQuote.js';
  let { venue, trader } = $props();
  const chain=$derived(store.values['dex.evm_chain_id|Venue|'+venue] ?? null);
  let input = $state('');
  let output = $state('');
  let amount = $state('');
  let busy = $state(false);
  let result = $state(null);
  let submitted = $state(null);
  let error = $state('');
  const address = /^0x[0-9a-fA-F]{40}$/;
  const valid = $derived(address.test(input) && address.test(output) && input.toLowerCase() !== output.toLowerCase()
    && /^\d+(?:\.\d+)?$/.test(amount) && /[1-9]/.test(amount));
  async function quote(event) {
    event.preventDefault();
    if (busy || !valid) return;
    result = null;
    error = '';
    busy = true;
    try {
      submitted = { input, output, amount, venue, chain, trader:trader.toLowerCase() };
      const quote = await post(`/api/venues/${seg(venue)}/sources/hashflow/quote`, { trader, input, output, amount_tokens: amount });
      if(sameHashflowWallet(submitted,venue,trader,chain,quote.details)) result=quote.details;
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }
</script>

<details>
  <summary>Котировка Hashflow</summary>
  {#if chain===1}
    <p>Для нескольких кошельков выберите API Multi-Wallet Taker. После старта укажите выданное имя в <code>hashflow.source</code>, а ключ в секрете <code>hashflow.taker_key</code>. Плательщик подписывает обмен через размещённый Hashflow Router. <a href="https://docs.hashflow.com/hashflow/taker/getting-started-api-v3">Получение доступа</a></p>
  {/if}
  <form onsubmit={quote}>
    <label>Контракт токена продажи <input bind:value={input} disabled={busy} placeholder="0x…" /></label>
    <label>Контракт токена покупки <input bind:value={output} disabled={busy} placeholder="0x…" /></label>
    <label>Сколько токенов продать <input bind:value={amount} disabled={busy} inputmode="decimal" placeholder="1.5" /></label>
    <button type="submit" disabled={busy || !valid}>{busy ? 'Проверяю…' : 'Запросить цену и проверить обмен'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result && submitted && sameHashflowWallet(submitted,venue,trader,chain,result)}
    <p role="status">За {submitted.amount} токена {submitted.input} Hashflow предложил {result.amount_out_tokens} токена {submitted.output}.
      Котировка {result.rfq_id}. Кошелёк {result.trader}, сеть {result.chain_id}, пул {result.pool}, Router {result.target}.
      Срок котировки — {new Date(result.expires * 1000).toLocaleTimeString()}. Газ — {result.gas_used} ед.</p>
  {/if}
</details>

<style>
  form { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  input { min-width: 0; }
  button { justify-self: start; }
</style>
