<!--
  Файл: ui/src/pages/Settings.svelte
  Что делает: экран «Настройки» по категориям (поле group схемы GET /api/settings/schema):
    адрес #/settings/<категория> — только настройки этой категории; #/settings — все категории.
    Слева — список категорий с числом настроек и не заданных нигде (переключение без возврата в меню,
    «Назад» браузера работает). Поиск по имени и описанию — всегда во ВСЕХ категориях, результаты по категориям.
    «Только незаданные нигде» — в открытой категории или (переключатель) во всех.
    Карточка настройки: описание, единица, ограничение, значения по областям, изменение на лету, история.
    Значений по умолчанию нет: незаданные помечены «не задано».
  Кто вызывает: App.svelte.
  От чего зависит: lib/settings.svelte.js, lib/settingsGroups.js, lib/router.svelte.js,
    components/SettingCard.svelte, SettingsGroupList.svelte, ErrorText.svelte.
  Настройки: все из docs/SETTINGS.md.
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { groupStats, filterSettings } from '../lib/settingsGroups.js';
  import { route } from '../lib/router.svelte.js';
  import SettingCard from '../components/SettingCard.svelte';
  import SettingsGroupList from '../components/SettingsGroupList.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  let text = $state('');
  let onlyUnset = $state(false);
  /** «Только незаданные» во всех категориях, а не в открытой. */
  let unsetEverywhere = $state(false);

  const stats = $derived(groupStats(store.schema, store.values));
  /** Открытая категория из адреса; неизвестная категория — показывается ошибка, а не пустой экран. */
  const group = $derived(route.arg ?? '');
  const known = $derived(!group || stats.some((g) => g.group === group));
  const searching = $derived(text.trim() !== '');
  const shown = $derived(
    filterSettings(store.schema, store.values, {
      text,
      onlyUnset,
      group: onlyUnset && unsetEverywhere ? '' : group,
    }),
  );
  /** Показывать заголовки категорий: при поиске, во «всех категориях» и при «незаданных во всех». */
  const grouped = $derived(searching || !group || (onlyUnset && unsetEverywhere));
  const totalUnset = $derived(stats.reduce((n, g) => n + g.unset, 0));
</script>

<h1>Настройки{group && !searching ? ` — ${group}` : ''}</h1>
<ErrorText text={store.error} />
{#if store.schema}
  <p class="muted">
    Стартовых значений нет: не задана настройка — функция, которой она нужна, не работает.
    Всего настроек: {store.schema.settings.length}, не задано нигде: {totalUnset}.
  </p>
  <div class="layout">
    <SettingsGroupList {stats} current={group} />
    <section>
      <div class="filters">
        <input type="search" bind:value={text} placeholder="поиск во всех категориях" title="по имени и описанию настройки" />
        <label class="inline"><input type="checkbox" bind:checked={onlyUnset} /> только незаданные нигде</label>
        {#if group}
          <label class="inline"><input type="checkbox" bind:checked={unsetEverywhere} disabled={!onlyUnset} /> во всех категориях</label>
        {/if}
      </div>
      {#if !known}
        <p class="err">Категории «{group}» нет в схеме настроек — выберите категорию слева.</p>
      {/if}
      {#if searching}<p class="muted">Поиск по всем категориям.</p>{/if}
      {#each shown as [g, defs] (g)}
        {#if grouped}<h2>{g} <span class="muted small">({defs.length})</span></h2>{/if}
        {#each defs as def (def.name)}<SettingCard {def} />{/each}
      {:else}
        {#if known}<p class="muted">Ничего не найдено.</p>{/if}
      {/each}
    </section>
  </div>
{:else if !store.error}
  <p class="muted">Загружаю схему настроек…</p>
{/if}

<style>
  .layout { display: grid; grid-template-columns: 250px minmax(0, 1fr); gap: 16px; align-items: start; }
  .filters input[type='search'] { min-width: 320px; }
  .small { font-size: 14px; font-weight: 400; }
  @media (max-width: 760px) {
    .layout { grid-template-columns: minmax(0, 1fr); }
    .filters input[type='search'] { min-width: 0; width: 100%; }
  }
</style>
