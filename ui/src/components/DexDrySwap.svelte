<!--
  Файл: ui/src/components/DexDrySwap.svelte
  Что делает: «Сухая сделка DEX» (решение 121, C4) на экране «Ручное управление»: монета (тикер и адрес контракта в
    BSC), сторона (купить / продать на DEX), сумма USDT, режим (как сделка / через исполнитель ArbExecutor с подставленным
    кодом — решение 122, поле executor) и кнопка «Проверить своп» → POST /api/dex/dry-swap. Бот собирает
    маршрут и транзакцию своим сборщиком как для сделки и прогоняет eth_simulateV1 от кошелька — ничего не отправляет,
    nonce не тратит. Результат — таблицей: форма маршрута, пулы, выход по котировке / живому пересчёту / минимум
    (amountOutMin) / по симуляции, газ обмена и разрешений, сколько разрешений нужно, откуда вход в симуляции, блок,
    итог «годится» или причина отказа, заметки (что не измерено).
  Режим «цикл» (решение 124) — components/DexDryCycle.svelte: цикл DEX ↔ DEX из текущих сигналов, весь путь ядра
    (бюджет, размер, граница) и симуляция без отправки. Итог обоих режимов — components/DryResult.svelte.
  Кто вызывает: pages/Manual.svelte.
  От чего зависит: lib/api.js, components/{ErrorText, DryResult, DexDryCycle}.svelte.
  Настройки: нет своих (через бэкенд — exec.dex_slippage_pct, dex.*, smart.*, verify.*, dex.multicall3).
-->
<script>
  import { post } from '../lib/api.js';
  import ErrorText from './ErrorText.svelte';
  import DryResult from './DryResult.svelte';
  import DexDryCycle from './DexDryCycle.svelte';

  /** 'swap' — своп монеты; 'cycle' — цикл DEX ↔ DEX (решение 124). */
  let kind = $state('swap');

  let ticker = $state('');
  let contract = $state('');
  let side = $state('Buy');
  let usdt = $state('');
  /** 'trade' — как пойдёт сделка; 'executor' — через исполнитель ArbExecutor с подставленным кодом (решение 122). */
  let mode = $state('trade');
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
        executor: mode === 'executor',
      });
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<h2>Сухая сделка DEX — проверить свой своп без отправки</h2>
<p class="muted">
  Маршрут и транзакция собираются как для сделки и прогоняются симуляцией (eth_simulateV1) от кошелька бота. В сеть ничего
  не уходит, nonce не тратится. Не хватает входа у кошелька — он переводится в симуляции из пула вне маршрута.
  Режим «через исполнитель» (решение 122): маршрут собирается шагами своего контракта ArbExecutor, а его код и владелец
  подставляются в симуляцию на будущий адрес развёртывания — так исполнитель проверяется на живых пулах до развёртывания.
</p>
<div class="row">
  <label>Что проверить
    <select bind:value={kind}>
      <option value="swap">своп монеты</option>
      <option value="cycle">цикл DEX ↔ DEX</option>
    </select>
  </label>
</div>
{#if kind === 'cycle'}
  <DexDryCycle />
{:else}
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
  <label>Режим
    <select bind:value={mode}>
      <option value="trade">как пойдёт сделка</option>
      <option value="executor">через исполнитель (код подставлен, до развёртывания)</option>
    </select>
  </label>
  <button disabled={busy || !ticker.trim() || !contract.trim() || !usdt.trim()} onclick={check}>
    {busy ? 'Проверяю…' : 'Проверить своп'}
  </button>
</div>
<ErrorText text={error} />
<DryResult {res} />
{/if}
