<!--
  Файл: ui/src/pages/Journal.svelte
  Что делает: экран «Журнал» — записи GET /api/journal?limit=N[&before=ID], новые первыми, с листанием
    («Ещё» — следующая страница старше последней); живые записи из WebSocket (/api/ws, тип «journal»)
    добавляются сверху; при «lagged» журнал перечитывается с начала.
    Фильтры по уровню, виду и площадке — по загруженным записям; варианты фильтров берутся из самих записей.
    Размер страницы задаёт зритель (PageSize), значения по умолчанию нет.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/labels.js, components/PageSize.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { levelLabel, showTime } from '../lib/labels.js';
  import PageSize from '../components/PageSize.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  let limit = $state(null);
  let entries = $state([]);
  let more = $state(false);
  let error = $state('');
  let busy = $state(false);
  let level = $state('');
  let kind = $state('');
  let venue = $state('');
  let open = $state({});

  /** Читает страницу. Принимает: before — id, старше которого читать (или null — с начала). */
  async function load(before) {
    if (limit === null) return;
    busy = true;
    try {
      const page = await get(`/api/journal?limit=${limit}${before ? `&before=${before}` : ''}`);
      entries = before ? [...entries, ...page] : page;
      more = page.length === limit;
      error = '';
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  $effect(() => {
    load(null);
    return onLive((m) => {
      if (m.type === 'lagged') load(null);
      if (m.type === 'journal' && !entries.some((e) => e.id === m.data.id)) entries = [m.data, ...entries];
    });
  });

  /** Разные значения поля среди загруженных записей — варианты фильтра. */
  const distinct = (field) => [...new Set(entries.map((e) => e[field]).filter(Boolean))].sort();
  const shown = $derived(
    entries.filter(
      (e) => (!level || e.level === level) && (!kind || e.kind === kind) && (!venue || e.venue === venue),
    ),
  );
</script>

<h1>Журнал</h1>
<div class="row">
  <PageSize key="journal.limit" bind:value={limit} label="Записей на страницу:" />
  <select bind:value={level}>
    <option value="">все уровни</option>
    {#each distinct('level') as l (l)}<option value={l}>{levelLabel(l)}</option>{/each}
  </select>
  <select bind:value={kind}>
    <option value="">все виды</option>
    {#each distinct('kind') as k (k)}<option value={k}>{k}</option>{/each}
  </select>
  <select bind:value={venue}>
    <option value="">все площадки</option>
    {#each distinct('venue') as v (v)}<option value={v}>{v}</option>{/each}
  </select>
</div>
<p class="muted">Фильтры действуют на загруженные записи; чтобы искать глубже — «Ещё».</p>
<ErrorText text={error} />
{#if limit === null}
  <p class="muted">Укажите, сколько записей показывать за раз.</p>
{:else}
  <table>
    <thead><tr><th>№</th><th>Когда</th><th>Уровень</th><th>Вид</th><th>Площадка</th><th>Что</th></tr></thead>
    <tbody>
      {#each shown as e (e.id)}
        <tr class={`lvl-${e.level}`}>
          <td>{e.id}</td><td>{showTime(e.at)}</td><td>{levelLabel(e.level)}</td>
          <td><code>{e.kind}</code></td><td>{e.venue ?? '—'}</td>
          <td>
            {e.message}
            {#if e.data}
              <button class="quiet" onclick={() => (open[e.id] = !open[e.id])}>{open[e.id] ? 'скрыть' : 'данные'}</button>
              {#if open[e.id]}<pre>{JSON.stringify(e.data, null, 2)}</pre>{/if}
            {/if}
          </td>
        </tr>
      {:else}
        <tr><td colspan="6" class="muted">Записей нет.</td></tr>
      {/each}
    </tbody>
  </table>
  {#if more}<button disabled={busy} onclick={() => load(entries.at(-1).id)}>Ещё</button>{/if}
{/if}
