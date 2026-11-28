<script>
  import { post, seg } from '../lib/api.js';
  import { previewRequest } from '../lib/tokenlonQuote.js';
  let {venue, trader, wallet} = $props();
  let token = $state(''), buy = $state(false), size = $state(''), firm = $state(false);
  let minimum = $state(''), maximum = $state(''), busy = $state(false);
  let answer = $state(null), error = $state('');
  const body = $derived(previewRequest({token, buy, size, firm, minimum, maximum, wallet, trader}));

  async function request(event) {
    event.preventDefault();
    if (busy || !body) return;
    busy = true;
    error = '';
    answer = null;
    try {
      answer = await post(`/api/wallets/${seg(wallet)}/tokenlon/${seg(venue)}/quote`, body);
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }
</script>
<details>
  <summary>Проверить цену Tokenlon PMM</summary>
  <form onsubmit={request}>
    <fieldset disabled={busy}>
      <label>Токен в паре с USDT Ethereum <input bind:value={token} placeholder="0x…" /></label>
      <label>Направление <select bind:value={buy}><option value={false}>Продать токен за USDT</option><option value={true}>Купить токен за USDT</option></select></label>
      <label>Количество токена в минимальных единицах <input bind:value={size} inputmode="numeric" /></label>
      <label><input type="checkbox" bind:checked={firm} /> Запросить условия маркетмейкера для кошелька {wallet}</label>
      {#if firm}
        <label>Минимум к получению в единицах {buy ? 'токена' : 'USDT'} <input bind:value={minimum} inputmode="numeric" /></label>
        {#if buy}<label>Максимум к оплате в минимальных единицах USDT <input bind:value={maximum} inputmode="numeric" /></label>{/if}
      {/if}
      <button type="submit" disabled={!body || busy}>{busy ? 'Запрос…' : 'Получить расчёт'}</button>
    </fieldset>
  </form>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if answer}
    <p role="status">Расход {answer.spent_raw} ({answer.input}). К получению {answer.received_raw} ({answer.output}). Суммы в минимальных единицах активов.</p>
    {#if answer.expires}<p>Срок условий до {new Date(answer.expires * 1000).toLocaleString()}.</p>{/if}
  {/if}
</details>
<style>
  fieldset { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  button { justify-self: start; }
</style>
