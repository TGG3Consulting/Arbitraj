<script>
  import { post, seg } from '../lib/api.js';
  import { previewRequest } from '../lib/tokenlonQuote.js';
  let {venue, trader, wallet, markets} = $props();
  let index = $state(0), buy = $state(false), size = $state(''), firm = $state(false);
  let minimum = $state(''), maximum = $state(''), busy = $state(false);
  let answer = $state(null), error = $state('');
  const market = $derived(markets[index]);
  const body = $derived(previewRequest({token:market?.base_token, cash:market?.quote_token, buy, size, firm, minimum, maximum, wallet, trader}));

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
      <label>Пара Tokenlon <select bind:value={index}>{#each markets as pair, i}<option value={i}>{pair.base} / {pair.quote}</option>{/each}</select></label>
      {#if !markets.length}<p>Заполните пары и контракты Tokenlon в настройках этой сети.</p>{/if}
      <label>Направление <select bind:value={buy}><option value={false}>Продать {market?.base ?? "токен"} за {market?.quote ?? "валюту пары"}</option><option value={true}>Купить {market?.base ?? "токен"} за {market?.quote ?? "валюту пары"}</option></select></label>
      <label>{buy ? 'Количество токена до комиссии Tokenlon, в минимальных единицах' : 'Расход токена в минимальных единицах'} <input bind:value={size} inputmode="numeric" /></label>
      <label><input type="checkbox" bind:checked={firm} /> Запросить условия маркетмейкера для кошелька {wallet}</label>
      {#if firm}
        <label>Минимум к получению в единицах {buy ? market?.base : market?.quote} <input bind:value={minimum} inputmode="numeric" /></label>
        {#if buy}<label>Максимум к оплате в минимальных единицах {market?.quote} <input bind:value={maximum} inputmode="numeric" /></label>{/if}
      {/if}
      <button type="submit" disabled={!body || busy}>{busy ? 'Запрос…' : 'Получить расчёт'}</button>
    </fieldset>
  </form>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if answer}
    <p role="status">Расход {answer.spent_raw} ({answer.input}). К получению {answer.received_raw} ({answer.output}). Суммы в минимальных единицах активов.</p>
    {#if answer.details?.localSimulationConfigured === false}<p>Для автоматической оплаты укажите собственный узел Ethereum в tokenlon.local_rpc.</p>{/if}
    {#if answer.details?.stage === 'indicative'}<p>Предварительная цена. Для суммы после комиссии запросите условия маркетмейкера.</p>{/if}
    {#if answer.details?.stage === 'maker_order'}<p>Маркетмейкер подписал условия. Комиссия Tokenlon — {answer.fee?.amount_raw} минимальных единиц получаемого актива. Фактическое исполнение ещё предстоит.</p>{/if}
    {#if answer.details?.native_input}<p>Оплата с баланса ETH этого кошелька.</p>{:else if answer.details?.native_output}<p>Выплата поступает на баланс ETH этого кошелька.</p>{/if}
    {#if answer.expires}<p>Срок условий до {new Date(answer.expires * 1000).toLocaleString()}.</p>{/if}
  {/if}
</details>
<style>
  fieldset { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  button { justify-self: start; }
</style>
