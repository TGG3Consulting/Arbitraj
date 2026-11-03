<!--
  Файл: ui/src/pages/Wallets.svelte
  Что делает: экран «Маппинг кошельков» (решение 132). Две вкладки: «Кошельки» (components/WalletList.svelte — добавить
    кошелёк: имя + ключ; таблица: имя, адрес, BNB, USDT, исполнитель с «Развернуть исполнитель», «Проверить
    подключение», переименовать, удалить) и «Маппинг» (components/WalletMap.svelte — биржа → кошелёк, «DEX ↔ DEX»,
    «Пробы и сигналы», предупреждение «деньги смешиваются», история «было/стало»). Секреты заблокированы — экран
    говорит, что нужна разблокировка. Ключ на экран не приходит никогда (только имя секрета и адрес).
    Данные — GET /api/wallets; перечитываются по записи журнала wallets.changed и при переподключении.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, components/WalletList.svelte, components/WalletMap.svelte.
  Настройки: нет (кошельки и маппинг — свои таблицы с историей, не настройки).
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import WalletList from '../components/WalletList.svelte';
  import WalletMap from '../components/WalletMap.svelte';

  let view = $state(null);
  let error = $state('');
  let tab = $state('wallets');

  /** Перечитывает данные экрана. */
  async function load() {
    try {
      view = await get('/api/wallets');
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    load();
    return onLive((msg) => {
      const kind = msg.type === 'journal' ? msg.data?.kind : null;
      if (kind === 'wallets.changed' || kind === 'secrets' || lagFor(msg, 'journal')) load();
    });
  });
</script>

<h1>Маппинг кошельков</h1>
<p class="muted">
  У каждой биржи — свой кошелёк DEX: свои деньги, nonce, разрешения (approve) и исполнитель. Биржа без кошелька в маппинге
  с DEX не торгует; циклы DEX ↔ DEX без своей строки не исполняются. DEX — одна площадка: пулы, котировки и настройки
  общие.
</p>
{#if error}<p class="err">{error}</p>{/if}
{#if view && !view.unlocked}
  <p class="err">Секреты заблокированы — нужна разблокировка (<a href="#/unlock">«Разблокировка»</a>): без неё адресов,
    балансов и добавления кошельков нет.</p>
{/if}

<div class="tabs">
  <button class:active={tab === 'wallets'} onclick={() => (tab = 'wallets')}>Кошельки</button>
  <button class:active={tab === 'map'} onclick={() => (tab = 'map')}>Маппинг</button>
  <button onclick={load}>Обновить</button>
</div>

{#if !view}
  <p class="muted">Загружаю…</p>
{:else if tab === 'wallets'}
  <WalletList {view} onchange={load} />
{:else}
  <WalletMap {view} onchange={load} />
{/if}

<style>
  .tabs {
    display: flex;
    gap: 0.5rem;
    margin: 0.5rem 0 1rem;
  }
  .tabs .active {
    font-weight: bold;
    text-decoration: underline;
  }
</style>
