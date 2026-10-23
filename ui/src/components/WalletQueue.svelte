<!--
  Файл: ui/src/components/WalletQueue.svelte
  Что делает: очередь транзакций кошелька площадки DEX (решение 45) на экране «Каналы отправки»: свободна /
    ждёт итога (nonce, хеш, с какого времени) / остановлена до ручной проверки (причина) и кнопка
    «Снять очередь после ручной проверки» (POST /api/wallet-queue/{venue}/release). Пока очередь остановлена,
    отправки DEX не идут; снимать — только проверив кошелёк и транзакцию в обозревателе блоков.
  Кто вызывает: pages/Channels.svelte.
  От чего зависит: lib/api.js, lib/labels.js (showTime).
  Настройки: send.cancel_max_gas_gwei, send.cancel_max_attempts, send.queue_max_wait_s (их задают на «Площадках»).
-->
<script>
  import { get, post, seg } from '../lib/api.js';
  import { showTime } from '../lib/labels.js';

  /** venue — id площадки DEX. */
  let { venue } = $props();
  let queue = $state(null);
  let error = $state('');
  let busy = $state(false);

  /** Перечитывает состояние очереди. */
  async function load() {
    error = '';
    try {
      queue = await get(`/api/wallet-queue/${seg(venue)}`);
    } catch (e) {
      error = e.message;
    }
  }

  /** «Снять очередь после ручной проверки». */
  async function release() {
    if (!confirm('Кошелёк и транзакция проверены вручную? Очередь станет свободной, отправки DEX продолжатся.')) return;
    busy = true;
    error = '';
    try {
      queue = await post(`/api/wallet-queue/${seg(venue)}/release`, {});
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    load();
  });

  const state = $derived(queue?.state);
  const halted = $derived(state && typeof state === 'object' && state.Halted ? state.Halted : null);
  const waiting = $derived(state && typeof state === 'object' && state.Busy ? state.Busy : null);
</script>

<article class="card">
  <header><strong>Очередь кошелька</strong> <button onclick={load}>Обновить</button></header>
  {#if error}
    <p class="err">{error}</p>
  {:else if !queue}
    <p class="muted">Загружаю…</p>
  {:else if halted}
    <p class="err">Остановлена до ручной проверки · {showTime(halted.since)}</p>
    <p>{halted.reason}</p>
    {#if halted.hash}<p class="muted">nonce {halted.nonce}, транзакция <code>{halted.hash}</code></p>{/if}
    <button disabled={busy} onclick={release}>Снять очередь после ручной проверки</button>
  {:else if waiting}
    <p>Ждёт итога транзакции <code>{waiting.hash}</code> (nonce {waiting.nonce}) · с {showTime(waiting.since)}</p>
  {:else}
    <p class="ok">Свободна</p>
  {/if}
</article>
