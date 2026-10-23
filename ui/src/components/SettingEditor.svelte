<!--
  Файл: ui/src/components/SettingEditor.svelte
  Что делает: значение одной настройки в одной области — показ («не задано», если нет), ввод,
    «Сохранить» и «Очистить» (PUT /api/settings; value:null — очистить). Ошибка бэкенда — как есть.
    Ввод по виду значения: Bool — кнопки «да»/«нет»; TextList — галочки из options или список через запятую;
    Integer — число (не целое уходит как текст, бэкенд объяснит ошибку); Decimal — строкой (без потери точности);
    остальное — текст. Значения по умолчанию нет: поле показывает текущее значение или пусто.
  Кто вызывает: components/SettingCard.svelte, pages/Notifications.svelte, pages/Venues.svelte.
  От чего зависит: lib/settings.svelte.js, lib/labels.js, components/ErrorText.svelte.
  Настройки: любая — по описанию из схемы.
-->
<script>
  import { store, valueKey, setValue } from '../lib/settings.svelte.js';
  import { showValue } from '../lib/labels.js';
  import ErrorText from './ErrorText.svelte';

  /** def — описание из схемы; scope — {kind, id?}; label — подпись области; options — варианты для TextList. */
  let { def, scope, label = '', options = null } = $props();

  const current = $derived(store.values[valueKey(def.name, scope)]);
  let draft = $state(null);
  let picked = $state(null);
  let error = $state('');
  let busy = $state(false);

  /** Текущее значение в виде текста для поля ввода. */
  const raw = $derived(
    current === undefined ? '' : Array.isArray(current) ? current.join(', ') : String(current),
  );
  const chosen = $derived(picked ?? (Array.isArray(current) ? current : []));

  /** Текст поля → значение для бэкенда по виду настройки. */
  function parse(text) {
    const t = text.trim();
    if (def.kind === 'Integer') return /^\d+$/.test(t) ? Number(t) : t;
    if (def.kind === 'TextList') return t ? t.split(',').map((s) => s.trim()).filter(Boolean) : [];
    return t;
  }

  /** Сохраняет значение (null — очистить). Ошибка бэкенда показывается под полем. */
  async function save(value) {
    busy = true;
    error = '';
    try {
      await setValue(def.name, scope, value);
      draft = null;
      picked = null;
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }

  /** Переключает вариант списка в черновике. */
  function toggle(opt) {
    picked = chosen.includes(opt) ? chosen.filter((o) => o !== opt) : [...chosen, opt];
  }
</script>

<div class="editor">
  {#if label}<span class="scope">{label}</span>{/if}
  <span class={current === undefined ? 'unset' : 'value'}>
    {current === undefined ? 'не задано' : showValue(current)}
  </span>
  {#if def.kind === 'Bool'}
    <button disabled={busy || current === true} onclick={() => save(true)}>да</button>
    <button disabled={busy || current === false} onclick={() => save(false)}>нет</button>
  {:else if def.kind === 'TextList' && options}
    {#each options as opt (opt)}
      <label class="inline"><input type="checkbox" checked={chosen.includes(opt)} onchange={() => toggle(opt)} />{opt}</label>
    {:else}
      <span class="muted">нет вариантов</span>
    {/each}
    <button disabled={busy || picked === null} onclick={() => save(picked)}>Сохранить</button>
  {:else}
    <input
      value={draft ?? raw}
      oninput={(e) => (draft = e.currentTarget.value)}
      placeholder={def.kind === 'TextList' ? 'через запятую' : def.unit}
      onkeydown={(e) => e.key === 'Enter' && draft !== null && save(parse(draft))}
    />
    <button disabled={busy || draft === null} onclick={() => save(parse(draft))}>Сохранить</button>
  {/if}
  {#if draft !== null || picked !== null}<span class="muted">не сохранено</span>{/if}
  <button class="quiet" disabled={busy || current === undefined} onclick={() => save(null)}>Очистить</button>
</div>
<ErrorText text={error} />
