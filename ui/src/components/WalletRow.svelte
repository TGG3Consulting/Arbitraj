<!--
  Файл: ui/src/components/WalletRow.svelte
  Что делает: карточка одного кошелька DEX (решение 132): имя, id и имя секрета ключа (ссылка на «Разблокировку» для
    замены), адрес (выводит адаптер из ключа) с «Копировать» или причина (секреты заблокированы, ключ не введён),
    строки маппинга, незавершённые сделки, BNB и USDT (GET /api/wallets/{id}/balances), «Проверить подключение»
    (POST /api/check/wallet/{id}), исполнитель ArbExecutor: адрес или «не развёрнут», «Развернуть исполнитель»
    (components/ExecutorDeploy.svelte), записать адрес уже развёрнутого (PUT /api/wallets/{id}/executor — только после
    проверки «код совпал, владелец — этот кошелёк»), версия исполнителя (решение 272: «v2» / «v1 — нативный BNB через
    форки не торгуется, переразверните» / «не сверена» — по коду в сети, поле executor_version; ревью 272 А1: у v2 —
    обёртка из конструктора executor_wrapped против dex.native_wrapped, не совпала — «BNB не идёт»; решение 395 (t181):
    «v3» — ещё шаги Uniswap V4 / PancakeSwap Infinity, у «v2» — подсказка «переразверните, чтобы смешанные с V4 /
    Infinity маршруты шли через исполнитель»; решение 399 (Н2): пока развёртывание запрещено — поле
    executor_deploy_refused, v3 не принят на живых пулах — вместо призыва «нужен исполнитель v3; v3 ещё не принят на
    живых пулах», а кнопка «Развернуть исполнитель» недоступна с этой причиной; решение 400 — классы шага v3 и их
    приёмка по записи симуляции, поле executor_v3_classes, таблица в ExecutorDeploy); «Переименовать» (PATCH), «Удалить» (DELETE — бэкенд запрещает,
    пока кошелёк в маппинге или в незавершённой сделке; ключ из секретов не удаляется).
  Кто вызывает: components/WalletList.svelte.
  От чего зависит: lib/api.js, components/{CheckConnection,ExecutorDeploy,CopyAddr,ErrorText}.svelte.
  Настройки: нет.
-->
<script>
  import { get, put, request, seg } from '../lib/api.js';
  import { flight } from '../lib/flight.js';
  import CheckConnection from './CheckConnection.svelte';
  import ExecutorDeploy from './ExecutorDeploy.svelte';
  import CopyAddr from './CopyAddr.svelte';
  import ErrorText from './ErrorText.svelte';

  /** w — кошелёк из GET /api/wallets; dex — id площадки DEX; unlocked — секреты разблокированы; onchange. */
  let { w, dex, unlocked, onchange } = $props();
  // Ревью 272 (А1): нативный BNB через исполнитель — v2 / v3 и обёртка в нём = dex.native_wrapped (как бот, native_block).
  let modern = $derived(w.executor_version === 'v2' || w.executor_version === 'v3');
  let nativeOk = $derived(modern && !!w.executor_wrapped && !!w.native_wrapped
    && w.executor_wrapped.toLowerCase() === w.native_wrapped.toLowerCase());
  // Решение 395 (t181): шаги Uniswap V4 / PancakeSwap Infinity через исполнитель — только у v3. Решение 399 (Н2): пока
  // развёртывание запрещено (executor_deploy_refused), без призыва переразвернуть — как бэкенд (deploy_gate.rs need_v3).
  // Решение 400: у v3 исполняются только классы шага, принятые записью симуляции на живых пулах (таблица ниже).
  let singletonNote = $derived(w.executor_version === 'v3'
    ? '; шаги Uniswap V4 и PancakeSwap Infinity в коде есть — исполняются только принятые классы (таблица «Классы шага v3»)'
    : `; шагов Uniswap V4 и PancakeSwap Infinity нет — маршрут, где они смешаны с другими DEX, этим кошельком не исполняется (${w.executor_deploy_refused ? 'нужен исполнитель v3; v3 ещё не принят на живых пулах' : 'переразверните исполнитель'})`);

  let bal = $state(null);
  let error = $state('');
  let exec = $state('');

  /** Балансы BNB и USDT кошелька (signal — отмена). Ошибка — в bal.error; отмена — ничего. */
  async function read(signal) {
    try {
      bal = await get(`/api/wallets/${seg(w.id)}/balances`, signal);
    } catch (e) {
      if (!e.aborted) bal = { error: e.message };
    }
  }
  /** Один запрос в полёте (lib/flight.js); смена кошелька или уход — отмена. */
  const balances = flight(read);

  /** Выполняет запрос и перечитывает экран; ошибка — красным. */
  async function act(fn) {
    error = '';
    try {
      await fn();
      onchange?.();
    } catch (e) {
      error = e.message;
    }
  }

  const rename = () => {
    const name = prompt('Новое имя кошелька', w.name);
    if (name) act(() => request('PATCH', `/api/wallets/${seg(w.id)}`, { name }));
  };
  const remove = () => {
    if (confirm(`Удалить кошелёк «${w.name}»? Ключ останется в секретах — удалить его можно на «Разблокировке».`))
      act(() => request('DELETE', `/api/wallets/${seg(w.id)}`));
  };
  const setExec = (address) => act(() => put(`/api/wallets/${seg(w.id)}/executor`, { address }));

  $effect(() => {
    if (unlocked && dex) balances();
    return () => balances.cancel();
  });
</script>

<article class="card">
  <header>
    <strong>{w.name}</strong> <span class="muted">({w.id}{w.legacy ? ', перенос прежнего кошелька' : ''})</span>
    <button onclick={rename}>Переименовать</button>
    <button onclick={remove} disabled={w.rows.length > 0 || w.open_deals.length > 0}
      title={w.rows.length ? 'Кошелёк в маппинге — сначала отвяжите' : w.open_deals.length ? 'Кошелёк ведёт незавершённые сделки' : ''}>
      Удалить
    </button>
  </header>
  <p>Адрес:
    {#if w.address}<CopyAddr addr={w.address} />{:else}<span class="err">{w.address_error}</span>{/if}
  </p>
  <p class="muted">Ключ: секрет <a href={`#/unlock/${seg(w.secret)}`}><code>{w.secret}</code></a></p>
  <p>Маппинг: {w.rows.length ? w.rows.join(', ') : 'не привязан'}
    {#if w.open_deals.length}· незавершённые сделки: {w.open_deals.join(', ')}{/if}</p>
  {#if bal}
    {#if bal.error}<p class="err">Балансы: {bal.error}</p>{/if}
    <p>BNB: {bal.bnb ?? '—'} · {bal.quote ?? 'USDT (venue.quote_asset не задана)'}: {bal.usdt ?? '—'}
      <button onclick={balances}>Обновить</button></p>
  {/if}
  <CheckConnection url={`/api/check/wallet/${seg(w.id)}`} />

  <h4>Исполнитель ArbExecutor</h4>
  <p>{#if w.executor}<code>{w.executor}</code> <button onclick={() => setExec(null)}>Снять</button>{:else}не развёрнут{/if}</p>
  {#if w.executor}
    <!-- Решение 272: версия по коду в сети (сверка — «Проверить подключение» исполнителя, сделка, развёртывание). -->
    <!-- Ревью 272 (А1): у v2 — обёртка из конструктора; ≠ dex.native_wrapped — BNB через исполнитель не идёт. -->
    <p class={nativeOk ? 'ok' : 'warn-text'}>Версия: {#if nativeOk}{w.executor_version} — нативный BNB монетой сделки идёт через исполнитель (обёртка <code>{w.executor_wrapped}</code>){singletonNote}{:else if modern}{w.executor_version}, но обёртка в исполнителе <code>{w.executor_wrapped ?? 'не прочитана'}</code> ≠ dex.native_wrapped <code>{w.native_wrapped ?? 'не задана'}</code> — нативный BNB через исполнитель не идёт (ERC-20 — как обычно), переразверните при верной настройке{singletonNote}{:else if w.executor_version === 'v1'}v1 — нативный BNB через форки не торгуется, переразверните{:else}не сверена — нажмите «Проверить подключение» исполнителя{/if}</p>
  {/if}
  {#if dex}<ExecutorDeploy venue={dex} wallet={w.id} refused={w.executor_deploy_refused} classes={w.executor_v3_classes ?? []} ondone={onchange} />{/if}
  <form class="inline" onsubmit={(e) => { e.preventDefault(); setExec(exec); exec = ''; }}>
    <input bind:value={exec} placeholder="адрес уже развёрнутого исполнителя (0x…)" />
    <button disabled={!exec}>Записать после проверки</button>
  </form>
  <ErrorText text={error} />
</article>
