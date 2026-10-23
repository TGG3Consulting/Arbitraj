<!--
  Файл: ui/src/pages/Settings.svelte
  Что делает: экран «Настройки» — все настройки из GET /api/settings/schema по группам, с описанием,
    единицей, ограничением и значениями по областям; изменение на лету; история. Фильтр по имени/описанию.
    Значений по умолчанию нет: незаданные помечены «не задано».
  Кто вызывает: App.svelte.
  От чего зависит: lib/settings.svelte.js, components/SettingCard.svelte, ErrorText.svelte.
  Настройки: все из docs/SETTINGS.md.
-->
<script>
  import { store, valueKey } from '../lib/settings.svelte.js';
  import SettingCard from '../components/SettingCard.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  let filter = $state('');
  let onlyUnset = $state(false);

  /** Задана ли настройка хоть для одной области. */
  const isSet = (d) => Object.keys(store.values).some((k) => k.startsWith(`${d.name}|`));

  const groups = $derived.by(() => {
    if (!store.schema) return [];
    const f = filter.trim().toLowerCase();
    const out = new Map();
    for (const d of store.schema.settings) {
      if (f && !d.name.toLowerCase().includes(f) && !d.description.toLowerCase().includes(f)) continue;
      if (onlyUnset && isSet(d)) continue;
      if (!out.has(d.group)) out.set(d.group, []);
      out.get(d.group).push(d);
    }
    return [...out];
  });
  const unsetGlobal = $derived(
    store.schema?.settings.filter(
      (d) => d.scopes.includes('Global') && store.values[valueKey(d.name, { kind: 'Global' })] === undefined,
    ).length ?? 0,
  );
</script>

<h1>Настройки</h1>
<ErrorText text={store.error} />
{#if store.schema}
  <p class="muted">
    Стартовых значений нет: не задана настройка — функция, которой она нужна, не работает.
    Не задано для всей системы: {unsetGlobal}.
  </p>
  <div class="row">
    <input bind:value={filter} placeholder="поиск по имени или описанию" />
    <label class="inline"><input type="checkbox" bind:checked={onlyUnset} /> только незаданные нигде</label>
  </div>
  {#each groups as [group, defs] (group)}
    <h2>{group}</h2>
    {#each defs as def (def.name)}<SettingCard {def} />{/each}
  {:else}
    <p class="muted">Ничего не найдено.</p>
  {/each}
{:else if !store.error}
  <p class="muted">Загружаю схему настроек…</p>
{/if}
