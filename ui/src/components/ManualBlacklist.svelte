<!--
  Файл: ui/src/components/ManualBlacklist.svelte
  Что делает: ручной чёрный список монет (решение 36) на экране «Монеты»: монеты, где coin.blocked = да,
    с причиной (coin.blocked_note); «Добавить» (адрес или тикер + причина) и «Убрать» (обе настройки очищаются).
    Решение 211 (монета = адрес контракта): отметка ставится на область «coin:0x…» (адрес в нижнем регистре);
    введённый тикер переводится в адрес по справочнику имён (lib/coinNames.svelte.js) — только если у тикера ровно
    один адрес, иначе — просьба ввести адрес. В списке — имя монеты («NMT / NETMIND») и адрес. Старые отметки по
    тикеру, которые миграция не перенесла (у тикера несколько адресов или нет ни одного), показаны отдельно: они
    продолжают блокировать все монеты с этим тикером, пока человек не задаст отметку по адресу и не уберёт старую.
    Работает через обычный PUT /api/settings (область «монета») — от ядра не зависит; изменения видны
    сразу во всех вкладках (живой поток настроек) и пишутся в историю настроек.
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/settings.svelte.js, lib/coinNames.svelte.js, components/ErrorText.svelte.
  Настройки: coin.blocked, coin.blocked_note (src/settings/catalog/trade.rs; контракт — src/api/README.md).
-->
<script>
  import { addressesOf } from '../lib/coinNames.svelte.js';
  import { store, setValue, defOf } from '../lib/settings.svelte.js';
  import { names, coinName, ensure, ADDR, coinIdOf } from '../lib/coinNames.svelte.js';
  import ErrorText from './ErrorText.svelte';

  // Справочник имён читается здесь напрямую (names.map) — загрузить его сразу (ревью М1).
  ensure();

  // Имена — из каталога бэкенда и контракта экрана «Монеты» (src/api/README.md).
  const BLOCKED = 'coin.blocked';
  const NOTE = 'coin.blocked_note';

  let input = $state('');
  let note = $state('');
  let error = $state('');
  let busy = $state(false);

  /** Отметки ручного списка: [{id, note, addr: bool}] по алфавиту. */
  const all = $derived.by(() => {
    const prefix = `${BLOCKED}|Coin|`;
    return Object.entries(store.values)
      .filter(([k, v]) => k.startsWith(prefix) && v === true)
      .map(([k]) => k.slice(prefix.length))
      .sort()
      .map((id) => ({ id, note: store.values[`${NOTE}|Coin|${id}`], addr: ADDR.test(id) }));
  });
  const list = $derived(all.filter((c) => c.addr));
  const legacy = $derived(all.filter((c) => !c.addr));

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

  /** Область отметки по вводу: адрес — как есть (нижний регистр); тикер — единственный адрес. Ошибка — текстом. */
  function scopeOf(v) {
    return { kind: 'Coin', id: coinIdOf(v) };
  }

  const add = () =>
    run(async () => {
      const scope = scopeOf(input.trim());
      await setValue(BLOCKED, scope, true);
      if (note.trim()) await setValue(NOTE, scope, note.trim());
      input = '';
      note = '';
    });

  const remove = (id) =>
    run(async () => {
      const scope = { kind: 'Coin', id };
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
    {#each list as c (c.id)}
      <div class="row">
        <strong>{names.map[c.id] ? coinName({ ticker: names.map[c.id].ticker, bsc_contract: c.id }) : 'нет в справочнике'}</strong>
        <code>{c.id}</code>
        <span class={c.note ? '' : 'muted'}>{c.note ?? 'причина не указана'}</span>
        <button class="quiet" disabled={busy} onclick={() => remove(c.id)}>Убрать</button>
      </div>
    {:else}
      <p class="muted">В ручном списке нет монет.</p>
    {/each}
    {#if legacy.length}
      <p class="warn-text">
        Старые отметки по тикеру (решение 211: не перенесены на адрес — у тикера несколько адресов или нет ни одного).
        Действуют на все монеты с этим тикером; задайте отметку по адресу и уберите старую.
      </p>
      {#each legacy as c (c.id)}
        {@const a = [...new Set(addressesOf(c.id))]}
        <div class="row">
          <strong>{c.id}</strong>
          <span class="muted">{a.length ? `адреса: ${a.join(', ')}` : 'монет с адресом под этим тикером нет'}</span>
          <span class={c.note ? '' : 'muted'}>{c.note ?? 'причина не указана'}</span>
          <button class="quiet" disabled={busy} onclick={() => remove(c.id)}>Убрать</button>
        </div>
      {/each}
    {/if}
    <form class="adder" onsubmit={(e) => { e.preventDefault(); add(); }}>
      <input bind:value={input} placeholder="адрес 0x… или тикер" />
      <input bind:value={note} placeholder="причина" />
      <button disabled={busy || !input.trim()}>Добавить</button>
    </form>
    <ErrorText text={error} />
  {/if}
</section>

