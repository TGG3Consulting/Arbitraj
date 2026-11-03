<!--
  Файл: ui/src/components/DexDryCycle.svelte
  Что делает: «Сухая сделка DEX» в режиме «цикл» (решение 124): «Загрузить циклы» — текущие сигналы циклов DEX ↔ DEX
    по каждой площадке-DEX (GET /api/venues/kinds → GET /api/venues/{id}/dex-cycles), выбор цикла (базовая монета,
    чистая прибыль на пробе, маршрут, исполним ли), свой бюджет USDT (необязательно — иначе лимиты money.*), режим (как
    пойдёт сделка / через исполнитель ArbExecutor с подставленным кодом) и «Проверить цикл» → POST /api/dex/dry-cycle.
    Бот проходит весь путь ядра (бюджет, подбор входа, порог, граница minOut) и собирает цикл как для сделки, затем
    eth_simulateV1 от кошелька — ничего не отправляет, nonce не тратит. Итог — components/DryResult.svelte.
  Кто вызывает: components/DexDrySwap.svelte.
  От чего зависит: lib/api.js, lib/fmt.js, lib/coinLabels.js (poolRefText), lib/labels.js (poolKindLabel),
    components/{ErrorText, DryResult}.svelte.
  Настройки: нет своих (через бэкенд — dex_dex.*, money.*, size.*, exec.*, verify.*).
-->
<script>
  import { get, post, seg } from '../lib/api.js';
  import { fmtPrice } from '../lib/fmt.js';
  import { poolRefText } from '../lib/coinLabels.js';
  import { poolKindLabel } from '../lib/labels.js';
  import ErrorText from './ErrorText.svelte';
  import DryResult from './DryResult.svelte';

  let cycles = $state([]);
  let pick = $state(-1);
  let budget = $state('');
  /** 'trade' — как пойдёт сделка; 'executor' — через исполнитель с подставленным кодом (решение 122). */
  let mode = $state('trade');
  let busy = $state(false);
  let error = $state('');
  let res = $state(null);

  /** Текущие сигналы циклов по всем площадкам-DEX. Ошибка — в error. */
  async function load() {
    error = '';
    try {
      const venues = await get('/api/venues/kinds');
      const lists = await Promise.all(
        venues.filter((v) => v.kind === 'Dex').map((v) => get(`/api/venues/${seg(v.id)}/dex-cycles`)),
      );
      cycles = lists.flat();
      pick = cycles.length ? 0 : -1;
      if (!cycles.length) error = 'Сигналов циклов нет: dex_dex.enabled выключено или циклов не найдено';
    } catch (e) {
      error = e.message;
    }
  }

  /** Сухая сделка выбранного цикла; ответ (с причиной отказа внутри) или ошибка HTTP — ниже формы. */
  async function check() {
    error = '';
    res = null;
    busy = true;
    try {
      res = await post('/api/dex/dry-cycle', {
        cycle: cycles[pick],
        budget_usdt: budget.trim() || null,
        executor: mode === 'executor',
      });
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  /** Строка цикла в списке: базовая, чистая прибыль на пробе, маршрут. */
  const label = (c) =>
    `${c.base_ticker}: ${c.net_profit == null ? 'чистая —' : `чистая ${fmtPrice(c.net_profit)}`} на оптимальном входе ${fmtPrice(c.input)}` +
    `${c.executable ? '' : ' (сигнал)'} — ${c.hops.map((h) => `${poolKindLabel(h.kind)} ${poolRefText(h.pool)}`).join(' → ')}`;
</script>

<p class="muted">
  Цикл DEX ↔ DEX (решение 124): бот подбирает вход как для сделки (бюджет money.* или свой, баланс базовой монеты
  кошелька, шаг dex_dex.size_step_base), ставит границу minOut = вход + затраты + порог dex_dex.min_profit_usd, собирает
  цикл исполнителем ArbExecutor или бандлом и прогоняет симуляцию. В сеть ничего не уходит.
</p>
<div class="row">
  <button onclick={load}>Загрузить циклы</button>
  <label>Цикл (оптимальный вход сигнала)
    <select bind:value={pick} disabled={!cycles.length}>
      {#each cycles as c, i (i)}<option value={i}>{label(c)}</option>{/each}
    </select>
  </label>
  <label>Бюджет, USDT <input bind:value={budget} inputmode="decimal" size="10" placeholder="по money.*" /></label>
  <label>Режим
    <select bind:value={mode}>
      <option value="trade">как пойдёт сделка</option>
      <option value="executor">через исполнитель (код подставлен, до развёртывания)</option>
    </select>
  </label>
  <button disabled={busy || pick < 0} onclick={check}>{busy ? 'Проверяю…' : 'Проверить цикл'}</button>
</div>
<ErrorText text={error} />
<DryResult {res} cycle={true} />
