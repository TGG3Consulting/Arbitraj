<!--
  Файл: ui/src/pages/Venues.svelte
  Что делает: экран «Площадки» — площадки из реестра шлюза (GET /api/venues); по каждой:
    «Проверить подключение» (POST /api/check/venue/{id}), её секреты (GET /api/secrets/known, где scope = id),
    её настройки области «площадка» (из схемы — где scopes содержит Venue), по группам.
    Статусы соединений, задержки и разрывы появятся, когда бэкенд начнёт их отдавать (docs/UI.md).
  Кто вызывает: App.svelte.
  От чего зависит: lib/settings.svelte.js, lib/secrets.svelte.js, components/CheckConnection.svelte,
    SecretRow.svelte, SettingEditor.svelte.
  Настройки: все с областью Venue (venue.*, feed.*, profit.*, exec.*, money.max_per_trade_usd и др.).
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { secrets } from '../lib/secrets.svelte.js';
  import CheckConnection from '../components/CheckConnection.svelte';
  import SecretRow from '../components/SecretRow.svelte';
  import SettingEditor from '../components/SettingEditor.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const groups = $derived.by(() => {
    const out = new Map();
    for (const d of store.schema?.settings ?? []) {
      if (!d.scopes.includes('Venue')) continue;
      if (!out.has(d.group)) out.set(d.group, []);
      out.get(d.group).push(d);
    }
    return [...out];
  });
</script>

<h1>Площадки</h1>
<ErrorText text={store.error} />
{#each store.venues as id (id)}
  <section class="card venue">
    <h2>{id}</h2>
    <CheckConnection url={`/api/check/venue/${encodeURIComponent(id)}`} />

    <h3>Ключи</h3>
    {#each secrets.known.filter((k) => k.scope === id) as item (item.name)}
      <SecretRow {item} />
    {:else}
      <p class="muted">У площадки нет секретов в списке бэкенда.</p>
    {/each}

    <h3>Настройки площадки</h3>
    {#each groups as [group, defs] (group)}
      <h4>{group}</h4>
      {#each defs as def (def.name)}
        <div class="setting-line">
          <code>{def.name}</code> <span class="muted">{def.description}{def.unit ? ` (${def.unit})` : ''}</span>
          <SettingEditor {def} scope={{ kind: 'Venue', id }} />
        </div>
      {/each}
    {/each}
  </section>
{:else}
  <p class="muted">{store.schema ? 'В шлюзе нет зарегистрированных площадок.' : 'Загружаю…'}</p>
{/each}
