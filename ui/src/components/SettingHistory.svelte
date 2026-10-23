<!--
  Файл: ui/src/components/SettingHistory.svelte
  Что делает: история изменений одной настройки (GET /api/settings/history?limit=N&name=…):
    когда, область, было → стало, кто. Сколько записей — поле PageSize (без значения по умолчанию).
    Обновляется, когда по живому потоку приходит изменение этой настройки.
  Кто вызывает: components/SettingCard.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/labels.js, components/PageSize.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { showValue, showTime } from '../lib/labels.js';
  import PageSize from './PageSize.svelte';
  import ErrorText from './ErrorText.svelte';

  /** name — имя настройки. */
  let { name } = $props();
  let limit = $state(null);
  let rows = $state([]);
  let error = $state('');

  /** Читает историю; без размера страницы не читает. */
  async function load() {
    if (limit === null) return;
    try {
      rows = await get(`/api/settings/history?limit=${limit}&name=${encodeURIComponent(name)}`);
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    load();
    return onLive((m) => {
      if (m.type === 'lagged' || (m.type === 'settings' && m.data.name === name)) load();
    });
  });
</script>

<div class="history">
  <PageSize key="history.limit" bind:value={limit} label="Записей истории:" />
  {#if limit === null}
    <p class="muted">Укажите, сколько записей истории показывать.</p>
  {:else if rows.length === 0}
    <p class="muted">Изменений не было.</p>
  {:else}
    <table>
      <thead><tr><th>Когда</th><th>Область</th><th>Было</th><th>Стало</th><th>Кто</th></tr></thead>
      <tbody>
        {#each rows as r, i (i)}
          <tr>
            <td>{showTime(r.at)}</td><td>{r.scope}</td>
            <td>{showValue(r.old) || 'не задано'}</td><td>{showValue(r.new) || 'не задано'}</td><td>{r.by}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  <ErrorText text={error} />
</div>
