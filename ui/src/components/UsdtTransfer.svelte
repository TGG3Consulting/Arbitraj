<!--
  Файл: ui/src/components/UsdtTransfer.svelte
  Что делает: «Перевести USDT» (решение 176: интерфейс и Telegram равны) в панели «Балансы»: «Откуда» — биржи и
    кошельки DEX со свободными USDT (решение 132), «Куда» — кроме «откуда» (от кошелька — только биржи), сумма или «Всё»,
    «Проверить» — сводка ядра (можно вывести, комиссия и минимум вывода, сколько дойдёт), «Отправить» — подтверждение с
    суммой и направлением, затем та же команда ядра, что у кнопки Telegram: TransferUsdt (существующий перегон USDT).
    Итог — текстом ядра под формой.
  Кто вызывает: components/BalancesPanel.svelte.
  От чего зависит: lib/api.js; бэкенд GET /api/usdt-transfer/sources, POST /api/usdt-transfer/quote,
    POST /api/manual/command {"TransferUsdt": {...}} (src/api/usdt_transfer.rs, src/core/usdt_transfer.rs).
  Настройки: нет (балансы — из памяти панели, ui.balances_refresh_ms).
-->
<script>
  import { get, post } from '../lib/api.js';

  /** onDone — после отправки (панель обновит балансы). */
  let { onDone } = $props();
  let sources = $state([]);
  let error = $state('');
  let from = $state('');
  let to = $state('');
  let amount = $state('');
  let quote = $state(null);
  let busy = $state(false);
  let result = $state(null);

  const byKey = (k) => sources.find((s) => s.key === k);
  const froms = $derived(sources.filter((s) => s.usdt != null && Number(s.usdt) > 0));
  const tos = $derived.by(() => {
    const f = byKey(from);
    if (!f) return [];
    return sources.filter((s) => s.key !== f.key && (f.wallet == null || s.wallet == null));
  });

  /** Загружает площадки с USDT. Ошибка — в error. */
  async function load() {
    error = '';
    try {
      sources = (await get('/api/usdt-transfer/sources')).sources;
    } catch (e) {
      error = e.message;
    }
  }
  load();

  /** Тело запроса из выбранного. Принимает: сумму (null — «Всё»). */
  function body(sum) {
    const f = byKey(from);
    const t = byKey(to);
    return { from: f.venue, from_wallet: f.wallet, to: t.venue, to_wallet: t.wallet, amount: sum };
  }

  /** Сводка ядра. Принимает: сумму (null — «Всё»). Ошибки: в quote.error. */
  async function check(sum) {
    if (!byKey(from) || !byKey(to)) return;
    busy = true;
    result = null;
    try {
      quote = await post('/api/usdt-transfer/quote', body(sum));
      if (quote.ok && sum == null) amount = String(quote.quote.amount);
    } catch (e) {
      quote = { ok: false, error: e.message };
    }
    busy = false;
  }

  /** «Отправить»: подтверждение с суммой и направлением, затем команда ядра. Ошибки: в result. */
  async function send() {
    const q = quote?.quote;
    if (!q) return;
    const fee = q.fee == null ? 'газ BNB с кошелька' : `комиссия вывода ${q.fee} ${q.ticker}`;
    const text = `Перевести ${q.amount} ${q.ticker} с ${byKey(from).label} на ${byKey(to).label}? ${fee}, дойдёт ${q.arrives} ${q.ticker}, сеть BSC (${q.network}).`;
    if (!confirm(text)) return;
    busy = true;
    try {
      result = await post('/api/manual/command', { action: { TransferUsdt: body(String(q.amount)) } });
    } catch (e) {
      result = { ok: false, details: e.message };
    }
    busy = false;
    quote = null;
    await load();
    onDone?.();
  }
</script>

<div class="tr">
  {#if error}<p class="err small">{error}</p>{/if}
  <label>Откуда
    <select bind:value={from} onchange={() => { to = ''; quote = null; }}>
      <option value="">—</option>
      {#each froms as s (s.key)}<option value={s.key}>{s.label} — {s.usdt} USDT</option>{/each}
    </select>
  </label>
  <label>Куда
    <select bind:value={to} onchange={() => (quote = null)} disabled={!from}>
      <option value="">—</option>
      {#each tos as s (s.key)}<option value={s.key}>{s.label}</option>{/each}
    </select>
  </label>
  <label>Сколько USDT <input bind:value={amount} inputmode="decimal" oninput={() => (quote = null)} /></label>
  <div class="btns">
    <button disabled={busy || !to} onclick={() => check(null)}>Всё</button>
    <button disabled={busy || !to || !amount} onclick={() => check(amount)}>Проверить</button>
    <button disabled={busy || !quote?.ok} onclick={send}>{busy ? 'Жду…' : 'Отправить'}</button>
  </div>
  {#if quote?.ok}
    {@const q = quote.quote}
    <p class="small">
      Можно вывести {q.withdrawable} (свободно {q.available}); {q.fee == null ? 'газ BNB' : `комиссия ${q.fee}`};
      минимум {q.min}; дойдёт <strong>{q.arrives} {q.ticker}</strong>, сеть {q.network}{q.note ? `; ${q.note}` : ''}
    </p>
  {:else if quote}
    <p class="err small">{quote.error}</p>
  {/if}
  {#if result}<p class="small" class:err={!result.ok} class:ok={result.ok}>{result.details}</p>{/if}
</div>

<style>
  .tr {
    display: grid;
    gap: 0.3rem;
    padding: 0.4rem 0;
    border-bottom: 1px solid var(--line);
  }
  label {
    display: grid;
    grid-template-columns: 7rem 1fr;
    align-items: center;
    gap: 0.4rem;
  }
  .btns {
    display: flex;
    gap: 0.4rem;
  }
  .small {
    font-size: 0.8rem;
    margin: 0.15rem 0;
  }
  .ok {
    color: var(--ok, inherit);
  }
  .err {
    color: var(--err);
  }
</style>
