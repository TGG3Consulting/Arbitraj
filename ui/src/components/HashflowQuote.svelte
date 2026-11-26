<!-- Ручная котировка для адреса выбранного кошелька. Сумма остаётся десятичной строкой. -->
<script>
  import { post, seg } from '../lib/api.js';
  let { venue, trader } = $props();
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
      submitted = { input, output, amount };
      const quote = await post(`/api/venues/${seg(venue)}/sources/hashflow/quote`, { trader, input, output, amount_tokens: amount });
      result = quote.details;
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }
</script>

<details>
  <summary>Котировка Hashflow</summary>
  <form onsubmit={quote}>
    <label>Контракт токена продажи <input bind:value={input} disabled={busy} placeholder="0x…" /></label>
    <label>Контракт токена покупки <input bind:value={output} disabled={busy} placeholder="0x…" /></label>
    <label>Сколько токенов продать <input bind:value={amount} disabled={busy} inputmode="decimal" placeholder="1.5" /></label>
    <button type="submit" disabled={busy || !valid}>{busy ? 'Проверяю…' : 'Запросить цену и проверить обмен'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result && submitted}
    <p role="status">За {submitted.amount} токена {submitted.input} Hashflow предложил {result.amount_out_tokens} токена {submitted.output}.
      Котировка {result.rfq_id}.
      Срок котировки — {new Date(result.expires * 1000).toLocaleTimeString()}. Газ — {result.gas_used} ед.</p>
  {/if}
</details>

<style>
  form { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  input { min-width: 0; }
  button { justify-self: start; }
</style>
