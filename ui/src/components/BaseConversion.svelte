<!-- Проверка конкретной пары и направления Base на странице площадок. Запрос читает котировку. -->
<script>
  import { get } from '../lib/api.js';
  import { store } from '../lib/settings.svelte.js';
  let venue = $state('');
  let side = $state('Sell');
  let amount = $state('');
  let result = $state(null);
  let error = $state('');
  let busy = $state(false);
  const exchanges = $derived(store.venueKinds.filter((v) => v.kind === 'Cex'));
  function clear() { result = null; error = ''; }
  async function quote() {
    clear();
    busy = true;
    try {
      result = await get(`/api/venues/${encodeURIComponent(venue)}/base-usdc-conversion?${new URLSearchParams({ side, quantity: amount })}`);
    } catch (e) { error = e.message; }
    finally { busy = false; }
  }
</script>

<details>
  <summary>Конверсия Base USDC на бирже</summary>
  <fieldset disabled={busy}>
    <label>Биржа
      <select bind:value={venue} onchange={clear}>
        <option value="">Выберите биржу</option>
        {#each exchanges as v (v.id)}<option value={v.id}>{v.id}</option>{/each}
      </select>
    </label>
    <label>Направление
      <select bind:value={side} onchange={clear}>
        <option value="Sell">Продать USDC за USDT</option>
        <option value="Buy">Купить USDC за USDT</option>
      </select>
    </label>
    <label>Объём ордера, USDC <input bind:value={amount} oninput={clear} inputmode="decimal" /></label>
    <button disabled={!venue || !amount || busy} onclick={quote}>Рассчитать по стакану</button>
  </fieldset>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if result}
    <p>По стакану с комиссией {result.side === 'Buy' ? 'к списанию' : 'к зачислению'} {result.net_usdt} USDT.
      Ордер {result.order_usdc} USDC, предельная цена {result.limit_price} USDT за USDC.
      {result.side === 'Buy' ? 'Резерв' : 'Минимальная выручка'} по лимитной цене {result.limit_usdt} USDT.</p>
  {/if}
</details>
