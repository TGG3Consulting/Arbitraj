<!--
  Файл: ui/src/components/WalletRow.svelte
  Что делает: карточка одного кошелька DEX (решение 132): имя, id и имя секрета ключа (ссылка на «Разблокировку» для
    замены), адрес (выводит адаптер из ключа) с «Копировать» или причина (секреты заблокированы, ключ не введён),
    строки маппинга, незавершённые сделки, BNB и USDT (GET /api/wallets/{id}/balances), «Проверить подключение»
    (POST /api/check/wallet/{id}), исполнитель ArbExecutor: адрес или «не развёрнут», «Развернуть исполнитель»
    (components/ExecutorDeploy.svelte), записать адрес уже развёрнутого (PUT /api/wallets/{id}/executor — только после
    проверки «код совпал, владелец — этот кошелёк»); «Переименовать» (PATCH), «Удалить» (DELETE — бэкенд запрещает,
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
  {#if dex}<ExecutorDeploy venue={dex} wallet={w.id} ondone={onchange} />{/if}
  <form class="inline" onsubmit={(e) => { e.preventDefault(); setExec(exec); exec = ''; }}>
    <input bind:value={exec} placeholder="адрес уже развёрнутого исполнителя (0x…)" />
    <button disabled={!exec}>Записать после проверки</button>
  </form>
  <ErrorText text={error} />
</article>
