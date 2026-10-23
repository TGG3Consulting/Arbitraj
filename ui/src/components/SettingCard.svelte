<!--
  Файл: ui/src/components/SettingCard.svelte
  Что делает: карточка одной настройки на экране «Настройки»: имя, описание, единица, вид значения,
    ограничение, допустимые области и значения по областям:
    вся система; каждая площадка из GET /api/venues; монеты — уже заданные + ввод тикера;
    события — уже заданные + выбор кода из схемы. Кнопка «История».
  Кто вызывает: pages/Settings.svelte.
  От чего зависит: lib/settings.svelte.js, lib/labels.js, components/SettingEditor.svelte, SettingHistory.svelte.
  Настройки: любая — по описанию из схемы.
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { kindLabel, limitLabel, scopeLabel } from '../lib/labels.js';
  import SettingEditor from './SettingEditor.svelte';
  import SettingHistory from './SettingHistory.svelte';

  /** def — описание настройки из схемы. */
  let { def } = $props();
  let showHistory = $state(false);
  let added = $state({ Coin: [], Event: [] });
  let coinInput = $state('');
  let eventInput = $state('');

  /** Идентификаторы областей вида kind, для которых значение уже задано, плюс добавленные вручную. */
  function idsOf(kind) {
    const prefix = `${def.name}|${kind}|`;
    const set = new Set(added[kind]);
    for (const key of Object.keys(store.values)) if (key.startsWith(prefix)) set.add(key.slice(prefix.length));
    return [...set].sort();
  }

  /** Добавляет область для ввода значения (значение появится только после «Сохранить»). */
  function add(kind, id) {
    const v = id.trim();
    if (v && !added[kind].includes(v)) added[kind] = [...added[kind], v];
  }

  const eventTitle = (code) => store.schema.events.find((e) => e.code === code)?.title ?? code;
</script>

<article class="card">
  <header>
    <code>{def.name}</code>
    <span class="muted">
      {kindLabel(def.kind)}{def.unit ? `, ${def.unit}` : ''}{limitLabel(def.limit) ? `, ${limitLabel(def.limit)}` : ''}
      · области: {def.scopes.map(scopeLabel).join(', ')}
    </span>
  </header>
  <p>{def.description}</p>

  {#if def.scopes.includes('Global')}
    <SettingEditor {def} scope={{ kind: 'Global' }} label="вся система" />
  {/if}
  {#if def.scopes.includes('Venue')}
    {#each store.venues as v (v)}
      <SettingEditor {def} scope={{ kind: 'Venue', id: v }} label={`площадка ${v}`} />
    {:else}
      <p class="muted">Площадок в шлюзе нет — значение по площадке задать не для чего.</p>
    {/each}
  {/if}
  {#if def.scopes.includes('Coin')}
    {#each idsOf('Coin') as c (c)}
      <SettingEditor {def} scope={{ kind: 'Coin', id: c }} label={`монета ${c}`} />
    {/each}
    <div class="adder">
      <input bind:value={coinInput} placeholder="тикер монеты" />
      <button disabled={!coinInput.trim()} onclick={() => { add('Coin', coinInput); coinInput = ''; }}>
        Задать для монеты
      </button>
    </div>
  {/if}
  {#if def.scopes.includes('Event')}
    {#each idsOf('Event') as e (e)}
      <SettingEditor {def} scope={{ kind: 'Event', id: e }} label={eventTitle(e)} />
    {/each}
    <div class="adder">
      <select bind:value={eventInput}>
        <option value="">— событие —</option>
        {#each store.schema.events as ev (ev.code)}<option value={ev.code}>{ev.group}: {ev.title}</option>{/each}
      </select>
      <button disabled={!eventInput} onclick={() => { add('Event', eventInput); eventInput = ''; }}>
        Задать для события
      </button>
    </div>
  {/if}

  <button class="quiet" onclick={() => (showHistory = !showHistory)}>
    {showHistory ? 'Скрыть историю' : 'История'}
  </button>
  {#if showHistory}<SettingHistory name={def.name} />{/if}
</article>
