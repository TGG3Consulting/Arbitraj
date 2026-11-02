<!--
  Файл: ui/src/components/PoolDirectory.svelte
  Что делает: блок «Справочник пулов (индексатор)» площадки DEX на экране «Площадки» (решение 58): источник и сеть,
    подпись источника (если задана), почему не обновляется, сколько монет спрошено и с ошибками, сколько пулов с
    USDT/промежуточными и итог по ним, список DEX индексатора. Настройки index.* — ниже, в «Настройках площадки»
    (группа «Справочник пулов»); проверка индексатора — в «Проверить подключение» площадки. Решение 121: монеты, пулов
    которых индексатор не знает (404, сведение — не ошибка, C2), и поиск пулов по фабрикам на своём узле (C3,
    index.factory_scan): включён ли, когда проход, вызовов, найдено фабриками и из них взято после проверки на узле.
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
  /** Монеты, пулов которых индексатор не знает (404) — сведение, не ошибка (решение 121, C2). */
  const unknown = $derived(dir.tokens.filter((t) => t.note));
  /** Поиск по фабрикам (решение 121, C3); старый бэкенд без поля — пусто. */
  const fs = $derived(dir.factory ?? null);
  /** Пулы по итогу: [[подпись, число]]. */
  const byStatus = $derived.by(() => {
    const m = new Map();
    for (const p of dir.pools) {
      const s = p.status;
      const key = typeof s === 'string' ? dirStatus(s).text : s.NotConnected ? 'DEX не подключена' : s.Unsupported ? 'не поддержан' : 'отклонён на узле';
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
{#if unknown.length}
  <details>
    <summary>Индексатор не знает пулов монет: {unknown.length} (не ошибка)</summary>
    {#each unknown as t (t.token)}<p class="muted">{t.ticker}: {t.note}</p>{/each}
  </details>
{/if}
<h4>Поиск пулов по фабрикам на своём узле (index.factory_scan)</h4>
{#if !fs}
  <p class="muted">Бэкенд не отдаёт итог поиска по фабрикам.</p>
{:else}
  <ErrorText text={fs.error} />
  {#if !fs.enabled}
    <p class="muted">Выключен (index.factory_scan не задана или «нет»).</p>
  {:else}
    <p>
      {fs.at ? `Проход ${clock(fs.at)}` : 'Прохода ещё не было'}: вызовов {fs.calls}, найдено фабриками пулов {fs.found},
      из них взято после проверки на узле {fs.taken}{fs.failed ? `, без ответа узла ${fs.failed}` : ''}.
    </p>
    {#if fs.notes?.length}
      <details>
        <summary>По DEX: {fs.notes.length}</summary>
        {#each fs.notes as n, i (i)}<p class="muted">{n}</p>{/each}
      </details>
    {/if}
  {/if}
{/if}
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
