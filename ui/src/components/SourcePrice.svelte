<!-- Ручная проверка ответа авторизованного источника. -->
<script>
  import { post, seg } from '../lib/api.js';
  let { venue, trader, wallet, source = 'fynd', label = 'Fynd' } = $props();
  let input = $state(''), output = $state(''), amount = $state('');
  let firm = $state(false), minimum = $state(''), fee = $state('');
  let result = $state(null), error = $state(''), busy = $state(false);
  let builder = $state('buildernet');
  const address = /^0x[0-9a-fA-F]{40}$/;
  const valid = $derived(address.test(input) && address.test(output) && input.toLowerCase() !== output.toLowerCase() && /^[1-9]\d*$/.test(amount) && (!firm || (/^[1-9]\d*$/.test(minimum) && (source !== 'kipseli' || (/^\d{1,5}$/.test(fee))))));
  async function quote(event) {
    event.preventDefault();
    if (!valid || busy) return;
    busy = true;
    error = '';
    result = null;
    try {
      const terms = firm ? {wallet, minimum_raw: minimum, fee_tenths_bps: source === 'kipseli' ? Number(fee) : null} : undefined;
      result = await post(`/api/venues/${seg(venue)}/sources/${seg(source)}/quote`, { input, output, trader, amount_raw: amount, firm: terms, ...(source === 'bopamm' ? {builder} : {}) });
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }
</script>
<details>
  <summary>Проверить цену {label}</summary>
  <form onsubmit={quote}>
    <label>Адрес продаваемого токена <input bind:value={input} disabled={busy} placeholder="0x…" /></label>
    <label>Адрес получаемого токена <input bind:value={output} disabled={busy} placeholder="0x…" /></label>
    <label>Сумма в минимальных единицах входного токена <input bind:value={amount} disabled={busy} inputmode="numeric" /></label>
    {#if source === 'bopamm'}
      <label>Строитель Ethereum <select bind:value={builder} disabled={busy}><option value="buildernet">BuilderNet</option><option value="titan">Titan</option></select></label>
    {/if}
    {#if source !== 'fynd' && wallet}
      <label><input type="checkbox" bind:checked={firm} disabled={busy} /> Запросить индивидуальные условия для этого кошелька и проверить обмен в симуляции</label>
      {#if firm}
        <label>Минимальная выплата в единицах контракта <input bind:value={minimum} disabled={busy} inputmode="numeric" /></label>
        {#if source === 'kipseli'}<label>Согласованная комиссия Kipseli в десятых долях базисного пункта <input bind:value={fee} disabled={busy} inputmode="numeric" /></label>{/if}
      {/if}
    {/if}
    <button disabled={!valid || busy} type="submit">{busy ? 'Запрос…' : 'Запросить расчёт'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result && source !== 'fynd'}
    <p role="status">Оценка на блоке {result.block}. Расход {result.spent_raw} минимальных единиц {result.input}, выход {result.received_raw} минимальных единиц {result.output}, остаток входа {result.unspent_raw}.</p>
    {#if result.details.type === 'wallet_simulation'}<p>Кошелёк выполнил обмен в симуляции. Газ — {result.details.gas_used} ед. Минимальная выплата владельца — {result.details.owner_minimum_raw}.</p>{/if}
    {#if source === 'zeroex'}<p>Шаги 0x RFQ в расчёте агрегатора: {result.details.rfq_legs} из {result.details.legs.length}. Минимум API — {result.details.minimum_raw}.</p>{:else if source === 'bopamm'}<p>Строитель {result.details.builder}, блок {result.details.target_block}, слот {result.details.slot}. Срок кадра {new Date(result.expires * 1000).toISOString()}.</p>{:else if result.details.type !== 'wallet_simulation'}<p>Глубина книги до индивидуальной комиссии за подпись.</p>{/if}
  {:else if result}
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
