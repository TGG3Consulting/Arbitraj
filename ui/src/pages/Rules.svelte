<!--
  Файл: ui/src/pages/Rules.svelte
  Что делает: экран «Правила шлюза» — список правил (GET /api/rules): событие, область, действие,
    условие, включено ли; создание и изменение (components/RuleForm.svelte); выключение и включение
    (PUT /api/rules/{id} с enabled). Правило площадки сильнее общего (src/gateway/rules.rs).
    Изменения из других вкладок видны сразу: по записям журнала «rules.*» список перечитывается.
    Решение 230: правило, действие которого для его кода не действует (отказ вывода: повтор, «стоп»; «результат
    неизвестен»: повтор), помечено «не действует: причина» (GET /api/rules/meta, codes[].inactive).
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, components/RuleForm.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { uniqBy } from '../lib/uniq.js';
  import { get, put } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import { flight } from '../lib/flight.js';
  import RuleForm from '../components/RuleForm.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  let rules = $state([]);
  let meta = $state(null);
  let error = $state('');
  let editing = $state(undefined); // undefined — формы нет; null — новое правило; правило — изменение
  let rowError = $state('');

  /** Перечитывает правила и справочник (signal — отмена). Ошибка — в error; отмена — ничего. */
  async function read(signal) {
    try {
      [rules, meta] = await Promise.all([get('/api/rules', signal), get('/api/rules/meta', signal)]);
      error = '';
    } catch (e) {
      if (!e.aborted) error = e.message;
    }
  }
  /** Один запрос в полёте (lib/flight.js); уход со страницы — отмена. */
  const load = flight(read);

  $effect(() => {
    load();
    const off = onLive((m) => {
      if (lagFor(m, 'journal') || (m.type === 'journal' && m.data.kind.startsWith('rules.'))) load();
    });
    return () => {
      off();
      load.cancel();
    };
  });

  /** Включает или выключает правило. Ошибка бэкенда — под таблицей. */
  async function toggle(r) {
    rowError = '';
    try {
      await put(`/api/rules/${r.id}`, { ...r, enabled: !r.enabled });
      load();
    } catch (e) {
      rowError = `Правило №${r.id}: ${e.message}`;
    }
  }

  const codeTitle = (c) => meta?.codes.find((x) => x.code === c)?.title ?? '';
  /** Решение 230: причина, почему действие сохранённого правила не действует для его кода (meta codes[].inactive);
   *  пусто — действует. */
  const inactiveWhy = (r) => {
    const kind = typeof r.action === 'string' ? r.action : Object.keys(r.action)[0];
    return meta?.codes.find((x) => x.code === r.event)?.inactive?.[kind] ?? '';
  };
  /** Действие для показа: название и параметры. */
  function showAction(a) {
    const kind = typeof a === 'string' ? a : Object.keys(a)[0];
    const m = meta?.actions.find((x) => x.kind === kind);
    const p = typeof a === 'string' ? [] : Object.entries(Object.values(a)[0]);
    const params = p.map(([k, v]) => {
      const pm = m?.params.find((x) => x.name === k);
      return pm ? `${pm.title}: ${v} ${pm.unit}` : `${k}: ${v}`;
    });
    return [m?.title ?? kind, ...params].join('; ');
  }
  const done = () => { editing = undefined; load(); };
</script>

<h1>Правила шлюза</h1>
<p class="muted">Что делать при ошибке площадки. Правило площадки сильнее общего. Готовых правил нет — все вводятся здесь.</p>
<ErrorText text={error} />
{#if meta}
  {#if editing === undefined}
    <button onclick={() => (editing = null)}>Новое правило</button>
  {:else}
    {#key editing}<RuleForm {meta} rule={editing} {done} />{/key}
  {/if}
  {#if rules.length}
    <table>
      <thead><tr><th>№</th><th>Событие</th><th>Область</th><th>Действие</th><th>Условие</th><th>Состояние</th><th></th></tr></thead>
      <tbody>
        {#each uniqBy(rules, (r) => r.id) as r (r.id)}
          <tr class:off={!r.enabled}>
            <td>{r.id}</td>
            <td><code>{r.event}</code><br /><span class="muted">{codeTitle(r.event)}</span></td>
            <td>{r.scope.kind === 'Venue' ? `площадка ${r.scope.venue}` : 'общее'}</td>
            <td>{showAction(r.action)}{#if inactiveWhy(r)}<br /><span class="warn-text">не действует: {inactiveWhy(r)}</span>{/if}</td>
            <td>{#if r.condition}<code>{JSON.stringify(r.condition)}</code><br /><span class="warn-text">не применяется до этапа 5</span>{:else}—{/if}</td>
            <td>{r.enabled ? 'включено' : 'выключено'}</td>
            <td class="row">
              <button onclick={() => toggle(r)}>{r.enabled ? 'Выключить' : 'Включить'}</button>
              <button class="quiet" onclick={() => (editing = r)}>Изменить</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="muted">Правил нет. Без правила шлюз останавливает операцию и уведомляет.</p>
  {/if}
  <ErrorText text={rowError} />
{/if}
