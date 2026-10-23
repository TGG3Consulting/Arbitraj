<!--
  Файл: ui/src/components/DexFamilies.svelte
  Что делает: блок «DEX и виды пулов» площадки DEX на экране «Площадки»: по каждой DEX — включена ли
    (dex.enabled, id = имя DEX); по каждому виду пула — название, вид, ссылки на официальную документацию,
    включатели (да/нет — сразу видны, например dex.pool_enabled) и остальные настройки области «DEX» этого вида пула (адреса контрактов и др.).
    Список — от адаптера (GET /api/venues/{id}/dex-families), настройки — из схемы (scopes содержит Dex).
  Кто вызывает: pages/Venues.svelte.
  От чего зависит: lib/settings.svelte.js, lib/dex.svelte.js, lib/labels.js, components/SettingEditor.svelte.
  Настройки: dex.enabled, dex.pool_enabled, dex.factory, dex.router, dex.quoter и все прочие с областью Dex.
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { isDexLevel } from '../lib/dex.svelte.js';
  import { poolKindLabel } from '../lib/labels.js';
  import SettingEditor from './SettingEditor.svelte';

  /** families — [DexFamily] одной площадки. */
  let { families } = $props();

  const defs = $derived((store.schema?.settings ?? []).filter((d) => d.scopes.includes('Dex')));
  const dexDefs = $derived(defs.filter((d) => isDexLevel(d.name)));
  const familyDefs = $derived(defs.filter((d) => !isDexLevel(d.name)));
  /** Виды пулов по DEX: [[dex, [DexFamily]]]. */
  const byDex = $derived.by(() => {
    const out = new Map();
    for (const f of families) {
      if (!out.has(f.dex)) out.set(f.dex, []);
      out.get(f.dex).push(f);
    }
    return [...out];
  });
</script>

<h3>DEX и виды пулов</h3>
{#each byDex as [name, list] (name)}
  <div class="card">
    <h4>DEX {name}</h4>
    {#each dexDefs as def (def.name)}
      <SettingEditor {def} scope={{ kind: 'Dex', id: name }} label={def.description} />
    {/each}
    {#each list as f (f.family)}
      <div class="setting-line">
        <strong>{f.title}</strong> <code>{f.family}</code> <span class="tag">{poolKindLabel(f.kind)}</span>
        {#each f.docs as url (url)}
          <a href={url} target="_blank" rel="noreferrer noopener">документация</a>
        {/each}
        {#each familyDefs.filter((d) => d.kind === 'Bool') as def (def.name)}
          <SettingEditor {def} scope={{ kind: 'Dex', id: f.family }} label={def.description} />
        {/each}
        <details>
          <summary>Адреса контрактов и другие настройки вида пула</summary>
          {#each familyDefs.filter((d) => d.kind !== 'Bool') as def (def.name)}
            <div class="setting-line">
              <code>{def.name}</code> <span class="muted">{def.description}</span>
              <SettingEditor {def} scope={{ kind: 'Dex', id: f.family }} />
            </div>
          {/each}
        </details>
      </div>
    {/each}
  </div>
{:else}
  <p class="nodata">Нет данных: адаптер не сообщил ни одного вида пула.</p>
{/each}
