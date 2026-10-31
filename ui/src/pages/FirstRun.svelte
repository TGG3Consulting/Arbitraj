<!--
  Файл: ui/src/pages/FirstRun.svelte
  Что делает: экран «Первый запуск» — СОВЕТУЕМЫЕ стартовые значения настроек по разделам (GET /api/first-run).
    У каждой строки: имя настройки, область, готовое значение ИЛИ метод (для капитал-зависимых money.*/risk.*),
    обоснование и источник. Кнопка «Заполнить форму настроек» ПРЕДзаполняет поля на экране «Настройки»
    советуемыми значениями (у кого они есть) — Тигран смотрит и сохраняет сам, обычным путём, с журналом.
    НИКОГДА не сохраняет настройки автоматически. Значения — советы, не значения по умолчанию движка.
  Кто вызывает: App.svelte.
  От чего зависит: lib/remote.svelte.js, lib/firstRun.svelte.js, lib/router.svelte.js, lib/settings.svelte.js (href для ссылок на настройки),
    components/NoData.svelte, ErrorText.svelte.
  Настройки: нет своих (строки ссылаются на существующие настройки docs/SETTINGS.md).
-->
<script>
  import { remote } from '../lib/remote.svelte.js';
  import { setPrefill } from '../lib/firstRun.svelte.js';
  import { href } from '../lib/router.svelte.js';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const fr = remote('/api/first-run', []);

  /** Строки с готовым значением и областью — их можно предзаполнить в форме настроек. */
  function prefillable(data) {
    const out = [];
    for (const s of data.section)
      for (const r of s.rec)
        if (r.value != null && r.prefill)
          out.push({ name: r.name, scope: { kind: r.prefill.kind, id: r.prefill.id }, value: r.value });
    return out;
  }

  const canFill = $derived(fr.data ? prefillable(fr.data).length : 0);

  /** Кладёт советы в хранилище предзаполнения и открывает «Настройки». Сохраняет Тигран сам. */
  function fill() {
    setPrefill(prefillable(fr.data));
    location.hash = href('settings');
  }
</script>

<h1>Первый запуск</h1>
<NoData text={fr.noData} />
<ErrorText text={fr.error} />
{#if fr.loading}
  <p class="muted">Загружаю…</p>
{:else if fr.data}
  <div class="advisory">
    <strong>Это советы, а не готовые настройки.</strong>
    <p>{fr.data.note}</p>
    <p class="muted">{fr.data.source_note}</p>
    <button onclick={fill} disabled={!canFill}>
      Заполнить форму настроек{canFill ? ` (${canFill})` : ''}
    </button>
    <span class="muted small">
      Предзаполнит поля на экране «Настройки» советуемыми значениями — вы смотрите и сохраняете сами.
      Деньги и риск (метод, без готового числа) не предзаполняются — их вводите вы.
    </span>
  </div>

  {#each fr.data.section as s (s.title)}
    <section class="block">
      <h2>{s.title}</h2>
      <p class="muted">{s.intro}</p>
      {#each s.rec as r (r.name)}
        <article class="card">
          <header>
            <a href={href('settings')}><code>{r.name}</code></a>
            <span class="muted small">{r.area}{r.unit ? `, ${r.unit}` : ''}</span>
          </header>
          {#if r.value != null}
            <p class="val">Советуем: <strong>{r.value}</strong></p>
          {:else}
            <p class="method"><span class="tag">метод</span> {r.method}</p>
          {/if}
          <p class="muted">{r.rationale}</p>
          {#if r.source}<p class="muted small">Источник: {r.source}</p>{/if}
        </article>
      {/each}
    </section>
  {/each}
{/if}

<style>
  .advisory { border: 1px solid var(--line, #c9b458); background: color-mix(in srgb, #c9b458 12%, transparent); border-radius: 8px; padding: 12px 16px; margin-bottom: 20px; }
  .advisory p { margin: 6px 0; }
  .advisory button { margin: 8px 12px 6px 0; }
  .block { margin-bottom: 20px; }
  .block > .muted { margin-top: 0; }
  .card { margin: 8px 0; }
  .card header { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }
  .val { margin: 6px 0 4px; }
  .method { margin: 6px 0 4px; }
  .small { font-size: 14px; }
</style>
