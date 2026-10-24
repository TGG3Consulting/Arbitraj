<!--
  Файл: ui/src/components/DexFamilies.svelte
  Что делает: таблица сопоставления «DEX индексатора → вид пулов» площадки DEX (решение 58). Строка — DEX, как её
    называет индексатор (id — id области настроек «DEX»): имя, пулы монет в справочнике и итог по ним, вид (dex.kind:
    форк V2 / форк V3 / Infinity CL / Infinity Bin / Uniswap V4 / не поддерживается), включена ли (dex.pool_enabled);
    в «Адреса и настройки DEX» — фабрика, роутер, квотер, комиссия V2 и прочие настройки области «DEX». Списка DEX в
    интерфейсе нет: строки — DEX из справочника индексатора, DEX с заданным видом и области, где уже есть значения
    (например прежние id видов пулов). «Скопировать настройки из …» переносит незаданные значения dex.* из другой
    строки (например из прежнего id «pancakeswap_v3» в «pancakeswap-v3-bsc»).
  Кто вызывает: pages/Venues.svelte.
  От чего зависит: lib/settings.svelte.js, lib/directory.js, components/SettingEditor.svelte, ErrorText.svelte.
  Настройки: dex.kind, dex.pool_enabled, dex.factory, dex.router, dex.quoter, dex.universal_router и все прочие
    с областью Dex.
-->
<script>
  import { store, setValue, defOf } from '../lib/settings.svelte.js';
  import { dirStatus } from '../lib/directory.js';
  import SettingEditor from './SettingEditor.svelte';
  import ErrorText from './ErrorText.svelte';

  /** families — [DexFamily] (DEX с заданным видом); dir — PoolDirectory или null (справочник не получен). */
  let { families, dir } = $props();
  let showAll = $state(false);
  let copyFrom = $state({});
  let copyError = $state('');

  const defs = $derived((store.schema?.settings ?? []).filter((d) => d.scopes.includes('Dex')));
  const head = $derived(defs.filter((d) => d.name === 'dex.kind' || d.name === 'dex.pool_enabled'));
  const rest = $derived(defs.filter((d) => d.name !== 'dex.kind' && d.name !== 'dex.pool_enabled'));
  /** id областей «DEX», где уже есть хоть одно значение. */
  const withValues = $derived.by(() => {
    const out = new Set();
    for (const key of Object.keys(store.values)) {
      const [name, kind, id] = key.split('|');
      if (kind === 'Dex' && name.startsWith('dex.')) out.add(id);
    }
    return out;
  });
  /** Пулы справочника по DEX индексатора. */
  const poolsByDex = $derived.by(() => {
    const m = new Map();
    for (const p of dir?.pools ?? []) {
      if (!m.has(p.dex_id)) m.set(p.dex_id, []);
      m.get(p.dex_id).push(p);
    }
    return m;
  });
  /** Строки таблицы: [{id, name}] — по имени. */
  const rows = $derived.by(() => {
    const names = new Map((dir?.dexes ?? []).map((d) => [d.id, d.name]));
    const ids = new Set([...names.keys(), ...families.map((f) => f.family), ...withValues]);
    const list = [...ids].map((id) => ({ id, name: names.get(id) ?? '' }));
    const useful = (r) => withValues.has(r.id) || poolsByDex.has(r.id);
    return list.filter((r) => showAll || useful(r)).sort((a, b) => (a.name || a.id).localeCompare(b.name || b.id));
  });

  /** Итог по пулам DEX коротко: «взят 2, отклонён 1». */
  function summary(id) {
    const m = new Map();
    for (const p of poolsByDex.get(id) ?? []) {
      const s = typeof p.status === 'string' ? dirStatus(p.status).text : p.status.NotConnected ? 'не подключена' : p.status.Unsupported ? 'не поддержан' : 'отклонён';
      m.set(s, (m.get(s) ?? 0) + 1);
    }
    return [...m].map(([k, n]) => `${k}: ${n}`).join(', ');
  }

  /** Устаревшие значения области «DEX»: имени нет в каталоге (настройку удалили из кода). [{name, id, value}]. */
  const orphans = $derived(
    Object.entries(store.values)
      .map(([key, value]) => {
        const [name, kind, id] = key.split('|');
        return { name, kind, id, value };
      })
      .filter((x) => x.kind === 'Dex' && store.schema && !defOf(x.name)),
  );

  /** Удаляет устаревшее значение (бэкенд разрешает только очистку имени не из каталога). Ошибка — текстом. */
  async function dropOrphan(o) {
    copyError = '';
    try {
      await setValue(o.name, { kind: 'Dex', id: o.id }, null);
    } catch (e) {
      copyError = e.message;
    }
  }

  /** Копирует незаданные значения dex.* строки `from` в строку `to`. Ошибка — текстом. */
  async function copy(to) {
    const from = copyFrom[to];
    copyError = '';
    if (!from) return;
    try {
      for (const [key, value] of Object.entries(store.values)) {
        const [name, kind, id] = key.split('|');
        // Только настройки из каталога: устаревшие (dex.enabled, dex.v3_fee_tiers) не переносятся.
        if (kind !== 'Dex' || id !== from || !name.startsWith('dex.') || !defOf(name)) continue;
        if (store.values[`${name}|Dex|${to}`] !== undefined) continue;
        await setValue(name, { kind: 'Dex', id: to }, value);
      }
    } catch (e) {
      copyError = e.message;
    }
  }
</script>

<h3>DEX индексатора → вид пулов</h3>
<p class="muted">
  Пулы берутся только у DEX с видом и всеми адресами (иначе на «Монетах» — «DEX … не подключена: причина»).
  Адреса — с официальной страницы этой DEX. Без Universal Router пулы DEX в сделках не участвуют и не берутся.
</p>
<label><input type="checkbox" bind:checked={showAll} /> показать все DEX индексатора ({dir?.dexes.length ?? 0})</label>
<ErrorText text={copyError} />
<table class="map">
  <tbody>
    {#each rows as r (r.id)}
      <tr>
        <td>
          <strong>{r.name || r.id}</strong><br /><code>{r.id}</code>
          {#if !r.name}<span class="muted"> — нет в списке индексатора</span>{/if}
          {#if poolsByDex.has(r.id)}<br /><span class="muted">пулы монет: {summary(r.id)}</span>{/if}
        </td>
        <td>
          {#each head as def (def.name)}
            <SettingEditor {def} scope={{ kind: 'Dex', id: r.id }} label={def.name} />
          {/each}
        </td>
        <td>
          <details>
            <summary>Адреса и настройки DEX</summary>
            {#each rest as def (def.name)}
              <div class="setting-line">
                <code>{def.name}</code> <span class="muted">{def.description}</span>
                <SettingEditor {def} scope={{ kind: 'Dex', id: r.id }} />
              </div>
            {/each}
            <div class="setting-line">
              Скопировать незаданные настройки из
              <select bind:value={copyFrom[r.id]}>
                <option value="">—</option>
                {#each [...withValues].filter((x) => x !== r.id).sort() as src (src)}
                  <option value={src}>{src}</option>
                {/each}
              </select>
              <button class="quiet" disabled={!copyFrom[r.id]} onclick={() => copy(r.id)}>Скопировать</button>
            </div>
          </details>
        </td>
      </tr>
    {:else}
      <tr><td class="nodata">Нет строк: справочник индексатора ещё не получен и ни у одной DEX нет настроек.</td></tr>
    {/each}
  </tbody>
</table>

{#if orphans.length}
  <h4>Устаревшие настройки DEX (нет в каталоге — код их больше не читает)</h4>
  {#each orphans as o (o.name + o.id)}
    <div class="setting-line">
      <code>{o.name}</code> [<code>{o.id}</code>] = <code>{JSON.stringify(o.value)}</code>
      <span class="tag">устарела</span>
      <button class="quiet" onclick={() => dropOrphan(o)}>Удалить</button>
    </div>
  {/each}
{/if}

<style>
  .map td { vertical-align: top; padding: 6px 8px 6px 0; border-top: 1px solid var(--line); }
</style>
