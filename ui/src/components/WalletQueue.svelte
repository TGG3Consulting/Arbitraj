<!--
  Файл: ui/src/components/WalletQueue.svelte
  Что делает: очередь транзакций кошелька площадки DEX (решение 45) на экране «Каналы отправки»: свободна /
    ждёт итога (nonce, хеш, с какого времени) / остановлена до ручной проверки (причина; занят ли nonce в блоке) и
    кнопка «Снять очередь после ручной проверки» (POST /api/wallet-queue/{venue}/release). Если nonce остановленной
    транзакции ещё не занят — висящая транзакция может пройти позже: снятие только вторым подтверждением. Решение 132:
    очередь своя у каждого кошелька (?wallet=<id>; живое сообщение — по площадке и счёту)
    (pending_ok = true). Обновляется сам по живому сообщению wallet_queue (lib/live.svelte.js).
  Кто вызывает: pages/Channels.svelte (по кошельку из GET /api/wallets).
  От чего зависит: lib/api.js, lib/live.svelte.js (onLive), lib/labels.js (showTime).
  Настройки: send.cancel_max_gas_gwei, send.cancel_max_attempts, send.queue_max_wait_s (их задают на «Площадках»).
-->
<script>
  import { get, post, seg } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import { flight } from '../lib/flight.js';
  import { showTime } from '../lib/labels.js';

  /** venue — id площадки DEX; wallet — id кошелька (решение 132: очередь своя у кошелька); name — его имя. */
  let { venue, wallet, name } = $props();
  const q = $derived(`?wallet=${seg(wallet)}`);
  let queue = $state(null);
  let error = $state('');
  let busy = $state(false);
  /** Сервер ответил «nonce ещё не занят» — нужен второй шаг подтверждения. */
  let pending = $state('');

  /** Перечитывает состояние очереди (с проверкой nonce у остановленной; signal — отмена). */
  async function read(signal) {
    error = '';
    try {
      queue = await get(`/api/wallet-queue/${seg(venue)}${q}`, signal);
    } catch (e) {
      if (!e.aborted) error = e.message;
    }
  }
  /** Один запрос в полёте (lib/flight.js); смена кошелька или уход со страницы — отмена. */
  const load = flight(read);

  /** «Снять очередь после ручной проверки»; pendingOk — второй шаг при незанятом nonce. */
  async function release(pendingOk) {
    const ask = pendingOk
      ? 'Nonce ещё не занят: висящая транзакция может пройти позже. Всё равно снять очередь?'
      : 'Кошелёк и транзакция проверены вручную? Очередь станет свободной, отправки DEX продолжатся.';
    if (!confirm(ask)) return;
    busy = true;
    error = '';
    try {
      queue = await post(`/api/wallet-queue/${seg(venue)}/release${q}`, { pending_ok: pendingOk });
      pending = '';
    } catch (e) {
      if (e.status === 409) pending = e.message;
      else error = e.message;
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    load();
    const off = onLive((msg) => {
      if (msg.type === 'wallet_queue' && msg.data?.venue === venue && msg.data?.account === wallet) {
        queue = { ...msg.data, nonce_taken: null };
        if (msg.data.state?.Halted) load();
      } else if (lagFor(msg, 'core')) {
        load();
      }
    });
    return () => {
      off();
      load.cancel();
    };
  });

  const state = $derived(queue?.state);
  const halted = $derived(state && typeof state === 'object' && state.Halted ? state.Halted : null);
  const waiting = $derived(state && typeof state === 'object' && state.Busy ? state.Busy : null);
</script>

<article class="card">
  <header><strong>Очередь кошелька «{name}»</strong> <button onclick={load}>Обновить</button></header>
  {#if error}
    <p class="err">{error}</p>
  {/if}
  {#if !queue}
    <p class="muted">Загружаю…</p>
  {:else if halted}
    <p class="err">Остановлена до ручной проверки · {showTime(halted.since)}</p>
    <p>{halted.reason}</p>
    {#if halted.hash}<p class="muted">nonce {halted.nonce}, транзакция <code>{halted.hash}</code></p>{/if}
    {#if queue.nonce_taken === false}
      <p class="err">nonce {halted.nonce} ещё не занят — висящая транзакция может пройти позже</p>
    {:else if queue.nonce_taken === true}
      <p class="muted">nonce {halted.nonce} уже занят в блоке</p>
    {/if}
    {#if pending}
      <p class="err">{pending}</p>
      <button disabled={busy} onclick={() => release(true)}>Подтверждаю: снять при незанятом nonce</button>
    {:else}
      <button disabled={busy} onclick={() => release(false)}>Снять очередь после ручной проверки</button>
    {/if}
  {:else if waiting}
    <p>Ждёт итога транзакции <code>{waiting.hash}</code> (nonce {waiting.nonce}) · с {showTime(waiting.since)}</p>
  {:else}
    <p class="ok">Свободна</p>
  {/if}
</article>
