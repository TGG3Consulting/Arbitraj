<!--
  Файл: ui/src/components/PoolDirectory.svelte
  Что делает: блок «Справочник пулов (индексатор)» площадки DEX на экране «Площадки» (решение 58): источник и сеть,
    подпись источника (если задана), почему не обновляется, сколько монет спрошено и с ошибками, сколько пулов с
    USDT/промежуточными и итог по ним, список DEX индексатора. Настройки index.* — ниже, в «Настройках площадки»
    (группа «Справочник пулов»); проверка индексатора — в «Проверить подключение» площадки.
  Кто вызывает: pages/Venues.svelte.
  От чего зависит: lib/dex.svelte.js (reloadDex), lib/directory.js, lib/fmt.js, components/Attribution.svelte, ErrorText.svelte.
  Настройки: нет своих (index.* — область площадки).
-->
<script>
  import { reloadDex } from '../lib/dex.svelte.js';
  import { dirStatus } from '../lib/directory.js';
  import { clock } from '../lib/fmt.js';
  import Attribution from './Attribution.svelte';
  import ErrorText from './ErrorText.svelte';

  /** dir — PoolDirectory (GET /api/venues/{id}/pool-directory). */
  let { dir } = $props();
  const fetched = $derived(dir.tokens.filter((t) => t.fetched_at).length);
  const failed = $derived(dir.tokens.filter((t) => t.error));
  /** Пулы по итогу: [[подпись, число]]. */
  const byStatus = $derived.by(() => {
    const m = new Map();
    for (const p of dir.pools) {
      const s = p.status;
      const key = typeof s === 'string' ? dirStatus(s).text : s.NotConnected ? 'DEX не подключена' : 'отклонён на узле';
      m.set(key, (m.get(key) ?? 0) + 1);
    }
    return [...m];
  });
</script>

<h3>Справочник пулов (индексатор)</h3>
<p>
  Источник: <code>{dir.source ?? 'не задан (index.api_url)'}</code>, сеть <code>{dir.network ?? 'не задана (index.network)'}</code>.
  <button class="quiet" onclick={reloadDex}>Обновить экран</button>
</p>
<Attribution text={dir.attribution_text} url={dir.attribution_url} />
<ErrorText text={dir.error} />
<p>
  Монет спрошено: {fetched} из {dir.tokens.length}{failed.length ? `, с ошибкой: ${failed.length}` : ''};
  пулов с USDT и промежуточными: {dir.pools.length}{byStatus.length ? ' — ' : ''}{byStatus.map(([k, n]) => `${k}: ${n}`).join(', ')}.
</p>
{#if failed.length}
  <details>
    <summary>Ошибки по монетам</summary>
    {#each failed as t (t.token)}<p class="err">{t.ticker}: {t.error}</p>{/each}
  </details>
{/if}
<details>
  <summary>DEX индексатора: {dir.dexes.length}{dir.dexes_at ? ` (получен ${clock(dir.dexes_at)})` : ''}</summary>
  <p class="muted">{dir.dexes.map((d) => `${d.name} — ${d.id}`).join('; ')}</p>
</details>
