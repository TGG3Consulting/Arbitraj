<!--
  Файл: ui/src/components/WalletChain.svelte
  Что делает: решение 432 (шаг 4 двух сетей) — кошелёк DEX в одной сети: кошелёк (ключ, адрес) один на все сети, а
    баланс газа и USDT (GET /api/wallets/{id}/balances?venue=), «Проверить подключение» (POST
    /api/check/wallet/{id}?venue=), исполнитель ArbExecutor этой сети (адрес, версия, обёртка; «Снять», «Записать после
    проверки» — PUT /api/wallets/{id}/executor?venue=, «Развернуть исполнитель» — ExecutorDeploy с площадкой DEX сети)
    — у площадки DEX этой сети. Показывается, только когда площадок DEX две и больше (поле chains кошелька); одна —
    WalletRow прежний.
    Решение 491: запрет развёртывания и классы шага исполнителя — из строки сети c (c.executor_deploy_refused,
    c.executor_v3_classes), а не из общего поля кошелька (оно по площадке по умолчанию): иначе в блоке Arbitrum
    кнопка «Развернуть исполнитель» закрывалась/открывалась по статусу BSC.
  Кто вызывает: components/WalletRow.svelte.
  От чего зависит: lib/api.js, lib/flight.js, components/{CheckConnection,ExecutorDeploy,ErrorText}.svelte.
  Настройки: dex.chain, dex.native_wrapped (только показ).
-->
<script>
  import { get, put, seg } from '../lib/api.js';
  import { flight } from '../lib/flight.js';
  import { executorFeatures } from '../lib/walletExecutor.js';
  import CheckConnection from './CheckConnection.svelte';
  import HashflowQuote from './HashflowQuote.svelte';
  import ClipperQuote from './ClipperQuote.svelte';
  import SourcePrice from './SourcePrice.svelte';
  import AirSwapSource from './AirSwapSource.svelte';
  import DexalotSource from './DexalotSource.svelte';
  import ExecutorDeploy from './ExecutorDeploy.svelte';
  import ErrorText from './ErrorText.svelte';

  /** w — кошелёк из GET /api/wallets; c — его строка сети (chains[]); unlocked; onchange. */
  let { w, c, unlocked, onchange } = $props();
  const q = $derived(`?venue=${encodeURIComponent(c.venue)}`);
  const features = $derived(executorFeatures(c));

  let bal = $state(null);
  let error = $state('');
  let exec = $state('');

  /** Балансы кошелька в этой сети (signal — отмена). Ошибка — в bal.error. */
  async function read(signal) {
    try {
      bal = await get(`/api/wallets/${seg(w.id)}/balances${q}`, signal);
    } catch (e) {
      if (!e.aborted) bal = { error: e.message };
    }
  }
  const balances = flight(read);

  /** Записать или снять исполнитель этой сети; ошибка — красным. Принимает: адрес или null. */
  async function setExec(address) {
    error = '';
    try {
      await put(`/api/wallets/${seg(w.id)}/executor${q}`, { address });
      onchange?.();
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    if (unlocked) balances();
    return () => balances.cancel();
  });
</script>

<section>
  <h4>Сеть {c.chain ?? '(dex.chain не задана)'} — площадка <code>{c.venue}</code>{#if c.home} <span class="muted">(домашняя: исполнитель и очередь — прежние)</span>{/if}</h4>
  {#if bal}
    {#if bal.error}<p class="err">Балансы: {bal.error}</p>{/if}
    <p>Газ: {bal.bnb ?? '—'} · {bal.quote ?? 'USDT (venue.quote_asset не задана)'}: {bal.usdt ?? '—'}
      <button onclick={balances}>Обновить</button></p>
  {/if}
  <CheckConnection url={`/api/check/wallet/${seg(w.id)}${q}`} />
  {#if w.address && c.venue}<DexalotSource venue={c.venue} trader={w.address} wallet={w.id} />{/if}
  {#if w.address && c.venue}<AirSwapSource venue={c.venue} trader={w.address} wallet={w.id} />{/if}
  {#if w.address && c.venue}<HashflowQuote venue={c.venue} trader={w.address} />{/if}
  {#if w.address && c.venue && c.fynd}<SourcePrice venue={c.venue} trader={w.address} />{/if}
  {#if w.address && c.venue && c.zeroex}<SourcePrice venue={c.venue} trader={w.address} wallet={w.id} source="zeroex" label="0x" />{/if}
  {#if w.address && c.venue && c.textile}<SourcePrice venue={c.venue} trader={w.address} wallet={w.id} source="textile" label="Textile FX" />{/if}
  {#if w.address && c.venue && c.kipseli}<SourcePrice venue={c.venue} trader={w.address} wallet={w.id} source="kipseli" label="Kipseli" />{/if}
  {#if w.address && c.venue && c.clipper}<ClipperQuote venue={c.venue} trader={w.address} wallet={w.id} />{/if}
  {#if w.address && c.venue && c.bopamm}<SourcePrice venue={c.venue} trader={w.address} wallet={w.id} source="bopamm" label="bopAMM" />{/if}
  <p>Исполнитель: {#if c.executor}<code>{c.executor}</code> <button onclick={() => setExec(null)}>Снять</button>{:else}не развёрнут в этой сети{/if}</p>
  {#if c.executor}
    <p class={features.nativeOk ? 'ok' : 'warn-text'}>Версия: {#if features.nativeOk}{c.executor_version} — нативная монета идёт через исполнитель (обёртка <code>{c.executor_wrapped}</code>){:else if features.native}{c.executor_version}, но обёртка <code>{c.executor_wrapped ?? 'не прочитана'}</code> ≠ dex.native_wrapped <code>{c.native_wrapped ?? 'не задана'}</code>{:else if c.executor_version === 'v1'}v1 — переразверните{:else}не сверена — нажмите «Проверить подключение» исполнителя{/if}</p>
  {/if}
  <ExecutorDeploy venue={c.venue} wallet={w.id} executor={c.executor ?? null} refused={c.executor_deploy_refused} classes={c.executor_v3_classes ?? []} ondone={onchange} />
  <form class="inline" onsubmit={(e) => { e.preventDefault(); setExec(exec); exec = ''; }}>
    <input bind:value={exec} placeholder="адрес уже развёрнутого исполнителя в этой сети (0x…)" />
    <button disabled={!exec}>Записать после проверки</button>
  </form>
  <ErrorText text={error} />
</section>
