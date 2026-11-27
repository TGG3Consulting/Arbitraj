<!-- Ручная проверка ответа авторизованного источника. -->
<script>
  import { post, seg } from '../lib/api.js';
  import {priceBuilder} from '../lib/sourcePriceBuilder.js';
  let { venue, trader, wallet, source = 'fynd', label = 'Fynd', slotBuilders = null } = $props();
  let input = $state(''), output = $state(''), amount = $state('');
  let firm = $state(false), minimum = $state(''), fee = $state('');
  let result = $state(null), error = $state(''), busy = $state(false);
  let buyExact = $state(false), maxSpend = $state(''), allowTrade = $state(false);
  let builder = $state('');
  const channel = $derived(priceBuilder(source, slotBuilders, builder));
  const signedMaker = $derived(['textile','airswap','dexalot'].includes(source));
  const address = /^0x[0-9a-fA-F]{40}$/;
  const valid = $derived((!channel.required || Boolean(channel.selected)) && address.test(input) && address.test(output) && input.toLowerCase() !== output.toLowerCase() && /^[1-9]\d*$/.test(amount) && (!firm || (/^[1-9]\d*$/.test(minimum) && (source !== 'kipseli' || (/^\d{1,5}$/.test(fee))))));
  async function quote(event) {
    event.preventDefault();
    if (!valid || busy || (firm && buyExact && !/^[1-9]\d*$/.test(maxSpend))) return;
    busy = true;
    error = '';
    result = null;
    try {
      const terms = firm ? {wallet, minimum_raw: minimum, ...(signedMaker ? {allow_trade:allowTrade, ...(buyExact ? {maximum_raw:maxSpend} : {})} : {}), ...(source === 'kipseli' ? {fee_tenths_bps:Number(fee)} : {})} : undefined;
      result = await post(`/api/venues/${seg(venue)}/sources/${seg(source)}/quote`, { input, output, trader, ...(buyExact ? {buy_amount_raw:amount} : {amount_raw:amount}), firm: terms, ...(channel.required ? {builder:channel.selected} : {}) });
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
    {#if signedMaker}<label><input type="checkbox" bind:checked={buyExact} disabled={busy} /> Указать точный выход</label>{/if}
    <label>Сумма в минимальных единицах {buyExact ? 'выходного' : 'входного'} токена <input bind:value={amount} disabled={busy} inputmode="numeric" /></label>
    {#if channel.required}
      <label>Строитель Ethereum <select value={channel.selected ?? ''} onchange={(event) => builder = event.currentTarget.value} disabled={busy}>
        {#each channel.choices as name (name)}<option value={name}>{name === 'buildernet' ? 'BuilderNet' : name === 'titan' ? 'Titan' : name}</option>{/each}
      </select></label>
    {/if}
    {#if source !== 'fynd' && wallet}
      <label><input type="checkbox" bind:checked={firm} disabled={busy} /> {signedMaker ? 'Зарезервировать объём у маркетмейкера и проверить расчёт для этого кошелька' : channel.required ? 'Проверить обмен через модуль этого кошелька в слоте строителя' : 'Запросить индивидуальные условия для этого кошелька и проверить обмен в симуляции'}</label>
      {#if firm}
        <label>Минимальная выплата в единицах контракта <input bind:value={minimum} disabled={busy} inputmode="numeric" /></label>
        {#if signedMaker}
          {#if buyExact}<label>Максимальный расход в единицах контракта <input bind:value={maxSpend} disabled={busy} inputmode="numeric" /></label>{/if}
          <label><input type="checkbox" bind:checked={allowTrade} disabled={busy} /> Разрешить боту использовать эту заявку до истечения срока</label>
        {/if}
        {#if source === 'kipseli'}<label>Согласованная комиссия Kipseli в десятых долях базисного пункта <input bind:value={fee} disabled={busy} inputmode="numeric" /></label>{/if}
      {/if}
    {/if}
    <button disabled={!valid || busy} type="submit">{busy ? 'Запрос…' : 'Запросить расчёт'}</button>
  </form>
  {#if error}<p class="err" role="status">{error}</p>{/if}
  {#if result && source === 'airswap' && result.details.allow_trade}<p>Проверенная заявка доступна торговому ядру до {new Date(result.expires * 1000).toLocaleTimeString()}.</p>{/if}
  {#if result && source === 'textile'}
    <p>Комиссия включена в расход и составляет {result.fee?.amount_raw} единиц входного токена.</p>
    {#if result.details.owner_admitted}<p>Заявка доступна торговому ядру до {new Date(result.expires * 1000).toLocaleTimeString()}.</p>{/if}
  {/if}
  {#if result && source !== 'fynd'}
    <p role="status">{result.block == null ? 'Оценка по опубликованным уровням.' : 'Расчёт на блоке ' + result.block + '.'} Расход {result.spent_raw} минимальных единиц {result.input}, выход {result.received_raw} минимальных единиц {result.output}, остаток входа {result.unspent_raw}.</p>
    {#if result.details.type === 'wallet_simulation'}<p>Кошелёк выполнил обмен в симуляции. Газ — {result.details.gas_used} ед. {#if result.details.owner_minimum_raw != null}Минимальная выплата владельца — {result.details.owner_minimum_raw}.{/if}</p>{/if}
    {#if source === 'zeroex'}<p>Шаги 0x RFQ в расчёте агрегатора: {result.details.rfq_legs} из {result.details.legs.length}. Минимум API — {result.details.minimum_raw}.</p>{:else if channel.required}<p>Строитель {result.details.builder}, блок {result.details.target_block}, слот {result.details.slot}. Срок кадра {new Date(result.expires * 1000).toISOString()}.</p>{:else if source === 'dexalot' && result.details.type === 'indicative'}<p>Запрошена пара {result.details.requested_pair}. Dexalot рассчитал ориентир по {result.details.price_source_pair}. {result.details.physical_alias ? 'Физические активы этих пар различаются.' : ''}</p>{:else if source === 'airswap' && result.details.type === 'indicative'}<p>Индикативная цена маркетмейкера. Для проверки выплаты запросите подписанную заявку.</p>{:else if result.details.type !== 'wallet_simulation'}<p>Глубина книги до индивидуальной комиссии за подпись.</p>{/if}
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
