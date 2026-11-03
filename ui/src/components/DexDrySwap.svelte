<!--
  Файл: ui/src/components/DexDrySwap.svelte
  Что делает: «Сухая сделка DEX» (решение 121, C4) на экране «Ручное управление»: монета (тикер и адрес контракта в
    BSC), сторона (купить / продать на DEX), сумма USDT и кнопка «Проверить своп» → POST /api/dex/dry-swap. Бот собирает
    маршрут и транзакцию своим сборщиком как для сделки и прогоняет eth_simulateV1 от кошелька — ничего не отправляет,
    nonce не тратит. Результат — таблицей: форма маршрута, пулы, выход по котировке / живому пересчёту / минимум
    (amountOutMin) / по симуляции, газ обмена и разрешений, сколько разрешений нужно, откуда вход в симуляции, блок,
    итог «годится» или причина отказа, заметки (что не измерено).
  Кто вызывает: pages/Manual.svelte.
  От чего зависит: lib/api.js, lib/coinLabels.js (poolRefText), components/ErrorText.svelte.
  Настройки: нет своих (через бэкенд — exec.dex_slippage_pct, dex.*, smart.*, verify.*, dex.multicall3).
-->
<script>
  import { post } from '../lib/api.js';
  import { poolRefText } from '../lib/coinLabels.js';
  import ErrorText from './ErrorText.svelte';

  let ticker = $state('');
  let contract = $state('');
  let side = $state('Buy');
  let usdt = $state('');
  let busy = $state(false);
  let error = $state('');
  let res = $state(null);

  /** Отправляет запрос; ответ (с причиной отказа внутри) или ошибка HTTP — ниже формы. */
  async function check() {
    error = '';
    res = null;
    busy = true;
    try {
      res = await post('/api/dex/dry-swap', {
        asset: { ticker: ticker.trim(), bsc_contract: contract.trim() || null },
        side,
        usdt: usdt.trim(),
      });
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  /** Значение или «—» (не дошли до этого шага). */
  const v = (x) => (x === null || x === undefined ? '—' : x);
</script>

<h2>Сухая сделка DEX — проверить свой своп без отправки</h2>
<p class="muted">
  Маршрут и транзакция собираются как для сделки и прогоняются симуляцией (eth_simulateV1) от кошелька бота. В сеть ничего
  не уходит, nonce не тратится. Не хватает входа у кошелька — он переводится в симуляции из пула вне маршрута.
</p>
<div class="row">
  <label>Тикер <input bind:value={ticker} placeholder="CAKE" size="8" /></label>
  <label>Адрес в BSC <input bind:value={contract} placeholder="0x…" size="44" /></label>
  <label>Сторона
    <select bind:value={side}>
      <option value="Buy">купить на DEX (USDT → монета)</option>
      <option value="Sell">продать на DEX (монета → USDT)</option>
    </select>
  </label>
  <label>Сумма, USDT <input bind:value={usdt} inputmode="decimal" size="10" /></label>
  <button disabled={busy || !ticker.trim() || !contract.trim() || !usdt.trim()} onclick={check}>
    {busy ? 'Проверяю…' : 'Проверить своп'}
  </button>
</div>
<ErrorText text={error} />
{#if res}
  <p class={res.ok ? 'ok' : 'err'} role="status">
    {res.ok ? 'Годится: собрано, симуляция прошла, выход ≥ минимума, обмен сделали пулы маршрута.' : `Отказ: ${res.reason ?? 'без причины'}`}
  </p>
  <table>
    <tbody>
      <tr><th>Монета, сторона</th><td>{res.asset.ticker} <code>{res.asset.bsc_contract}</code>, {res.side === 'Buy' ? 'покупка' : 'продажа'} на {res.usdt} USDT</td></tr>
      <tr><th>Количество монет</th><td>{v(res.qty)}</td></tr>
      <tr><th>Форма маршрута</th><td>{v(res.form)}</td></tr>
      <tr><th>Пулы</th><td>
        {#each res.pools as p, i (i)}<div>доля {p.leg + 1}: {p.dex} ({p.kind}) <code>{poolRefText(p.pool)}</code></div>{:else}—{/each}
      </td></tr>
      <tr><th>Выход по котировке</th><td>{v(res.quote_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Выход по живому пересчёту</th><td>{v(res.live_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Минимум (amountOutMin)</th><td>{v(res.min_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Выход по симуляции</th><td>{v(res.sim_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Газ обмена / разрешений</th><td>{v(res.gas_swap)} / {v(res.gas_approve)} (разрешений: {res.approvals})</td></tr>
      <tr><th>Вход в симуляции</th><td>{v(res.funding)}</td></tr>
      <tr><th>Блок симуляции</th><td>{v(res.block)}</td></tr>
    </tbody>
  </table>
  {#each res.notes as n, i (i)}<p class="muted">{n}</p>{/each}
{/if}
