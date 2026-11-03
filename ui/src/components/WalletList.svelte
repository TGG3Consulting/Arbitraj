<!--
  Файл: ui/src/components/WalletList.svelte
  Что делает: вкладка «Кошельки» экрана «Маппинг кошельков» (решение 132): добавить кошелёк (имя + приватный ключ +
    пароль секретов → POST /api/wallets; ключ уходит сразу в зашифрованный файл и в поле не остаётся), таблица
    кошельков (components/WalletRow.svelte): имя, адрес, BNB, USDT, исполнитель, «Проверить подключение»,
    переименовать, удалить. Сообщения бэкенда — как есть.
  Кто вызывает: pages/Wallets.svelte.
  От чего зависит: lib/api.js, components/WalletRow.svelte, components/ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { post } from '../lib/api.js';
  import WalletRow from './WalletRow.svelte';
  import ErrorText from './ErrorText.svelte';

  /** view — ответ GET /api/wallets; onchange — перечитать экран. */
  let { view, onchange } = $props();

  let name = $state('');
  let key = $state('');
  let password = $state('');
  let busy = $state(false);
  let error = $state('');
  let done = $state('');

  /** Добавляет кошелёк; поля ключа и пароля очищаются при любом исходе. */
  async function add() {
    busy = true;
    error = '';
    done = '';
    try {
      const w = await post('/api/wallets', { name, key, password });
      done = `Кошелёк «${w.name}» добавлен`;
      name = '';
      onchange?.();
    } catch (e) {
      error = e.message;
    }
    key = '';
    password = '';
    busy = false;
  }
</script>

<h2>Добавить кошелёк</h2>
<form class="add" onsubmit={(e) => { e.preventDefault(); add(); }}>
  <label>Имя <input bind:value={name} placeholder="например MEXC" /></label>
  <label>Приватный ключ <input type="password" autocomplete="off" bind:value={key} placeholder="64 знака, можно с 0x" /></label>
  <label>Пароль секретов <input type="password" autocomplete="off" bind:value={password} /></label>
  <button disabled={busy || !name || !key || !password}>{busy ? 'Добавляю…' : 'Добавить'}</button>
</form>
<p class="muted">Ключ хранится только в зашифрованном файле секретов и на экран не возвращается. Кошелёк — отдельный, только
  под бота. Тот же ключ дважды добавить нельзя.</p>
<ErrorText text={error} />
{#if done}<p class="ok" role="status">{done}</p>{/if}

<h2>Кошельки</h2>
{#if !view.dex}
  <p class="err">Площадка DEX не подключена — кошельков нет.</p>
{/if}
{#each view.wallets as w (w.id)}
  <WalletRow {w} dex={view.dex} unlocked={view.unlocked} {onchange} />
{:else}
  <p class="muted">Кошельков нет. Прежний кошелёк бота переносится сам после разблокировки секретов (кошелёк «BingX»).</p>
{/each}

<style>
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: end;
  }
  .add label {
    display: flex;
    flex-direction: column;
  }
</style>
