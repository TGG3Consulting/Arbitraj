<!-- Ручная проверка ответа авторизованного источника. -->
<script>
  import { post, seg } from '../lib/api.js';
  let { venue, trader } = $props();
  let input = $state(''), output = $state(''), amount = $state('');
  let result = $state(null), error = $state(''), busy = $state(false);
  const address = /^0x[0-9a-fA-F]{40}$/;
  const valid = $derived(address.test(input) && address.test(output) && input.toLowerCase() !== output.toLowerCase() && /^[1-9]\d*$/.test(amount));
  async function quote(event) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    error = '';
    result = null;
    try {
      result = await post(`/api/venues/${seg(venue)}/sources/fynd/quote`, { input, output, trader, amount_raw: amount });
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }
</script>
<details>
  <summary>Проверить цену Fynd</summary>
  <form onsubmit={quote}>
    <label>Адрес продаваемого токена <input bind:value={input} disabled={busy} placeholder="0x…" /></label>
    <label>Адрес получаемого токена <input bind:value={output} disabled={busy} placeholder="0x…" /></label>
    <label>Сумма в минимальных единицах входного токена <input bind:value={amount} disabled={busy} inputmode="numeric" /></label>
    <button disabled={!valid || busy} type="submit">{busy ? 'Запрос…' : 'Запросить расчёт'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result}
    <p role="status">Расчёт на блоке {result.block}. Отставание в момент проверки — {result.details.age_blocks} блоков. За {result.spent_raw} минимальных единиц {result.input} выход до удержания комиссий составил {result.received_raw} минимальных единиц {result.output}. Шагов агрегатора через подтверждённые пулы Fynd AMM — {result.details.recognized_fynd_swaps} из {result.details.swaps}.</p>
    {#if result.details.minimum_received_raw != null}<p>Минимум по данным Fynd — {result.details.minimum_received_raw}. Комиссия роутера — {result.details.router_fee_raw}, клиента — {result.details.client_fee_raw}. Все суммы в минимальных единицах выходного токена.</p>{/if}
    {#if result.details.amount_out_net_gas_raw != null}<p>Оценка выхода за вычетом газа по данным Fynd — {result.details.amount_out_net_gas_raw}.</p>{/if}
  {/if}
</details>
<style>
  form { display: grid; gap: .5rem; max-width: 34rem; margin-block: .5rem; }
  label { display: grid; gap: .2rem; }
  button { justify-self: start; }
</style>
