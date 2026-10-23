<!--
  Файл: ui/src/components/ManualBlacklist.svelte
  Что делает: ручной чёрный список монет (решение 36) на экране «Монеты»: монеты, где coin.blocked = да,
    с причиной (coin.blocked_note); «Добавить» (тикер + причина) и «Убрать» (обе настройки очищаются).
    Работает через обычный PUT /api/settings (область «монета») — от ядра не зависит; изменения видны
    сразу во всех вкладках (живой поток настроек) и пишутся в историю настроек.
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/settings.svelte.js, components/ErrorText.svelte.
  Настройки: coin.blocked, coin.blocked_note (src/settings/catalog/trade.rs; контракт — src/api/README.md).
-->
<script>
  import { store, setValue, defOf } from '../lib/settings.svelte.js';
  import ErrorText from './ErrorText.svelte';

  // Имена — из каталога бэкенда и контракта экрана «Монеты» (src/api/README.md).
  const BLOCKED = 'coin.blocked';
  const NOTE = 'coin.blocked_note';

  let ticker = $state('');
  let note = $state('');
  let error = $state('');
  let busy = $state(false);

  /** Монеты из ручного списка: [{ticker, note}] по алфавиту. */
  const list = $derived.by(() => {
    const prefix = `${BLOCKED}|Coin|`;
    return Object.entries(store.values)
      .filter(([k, v]) => k.startsWith(prefix) && v === true)
      .map(([k]) => k.slice(prefix.length))
      .sort()
      .map((t) => ({ ticker: t, note: store.values[`${NOTE}|Coin|${t}`] }));
  });

  /** Выполняет изменения по очереди; ошибка бэкенда — под формой. Принимает: функцию. */
  async function run(fn) {
    busy = true;
    error = '';
    try {
      await fn();
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  const add = () =>
    run(async () => {
      const scope = { kind: 'Coin', id: ticker.trim() };
      await setValue(BLOCKED, scope, true);
      if (note.trim()) await setValue(NOTE, scope, note.trim());
      ticker = '';
      note = '';
    });

  const remove = (t) =>
    run(async () => {
      const scope = { kind: 'Coin', id: t };
      await setValue(BLOCKED, scope, null);
      await setValue(NOTE, scope, null);
    });
</script>

<section class="card">
  <h2>Ручной чёрный список</h2>
  {#if !defOf(BLOCKED) || !defOf(NOTE)}
    <p class="err">Настроек {BLOCKED} / {NOTE} нет в схеме бэкенда.</p>
  {:else}
    <p class="muted">{defOf(BLOCKED).description}</p>
    {#each list as c (c.ticker)}
      <div class="row">
        <strong>{c.ticker}</strong>
        <span class={c.note ? '' : 'muted'}>{c.note ?? 'причина не указана'}</span>
        <button class="quiet" disabled={busy} onclick={() => remove(c.ticker)}>Убрать</button>
      </div>
    {:else}
      <p class="muted">В ручном списке нет монет.</p>
    {/each}
    <form class="adder" onsubmit={(e) => { e.preventDefault(); add(); }}>
      <input bind:value={ticker} placeholder="тикер монеты" />
      <input bind:value={note} placeholder="причина" />
      <button disabled={busy || !ticker.trim()}>Добавить</button>
    </form>
    <ErrorText text={error} />
  {/if}
</section>
