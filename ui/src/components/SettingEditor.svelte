<!--
  Файл: ui/src/components/SettingEditor.svelte
  Что делает: значение одной настройки в одной области — показ («не задано», если нет), ввод,
    «Сохранить» и «Очистить» (PUT /api/settings; value:null — очистить). Ошибка бэкенда — как есть.
    Ввод по виду значения: Bool — кнопки «да»/«нет»; TextList — галочки из options или список через запятую;
    Integer — число (не целое уходит как текст, бэкенд объяснит ошибку), верхняя граница def.max проверяется
    до отправки; Decimal — строкой (без потери точности); Choice — выбор из def.choices (русские подписи);
    Address — адрес BSC, формат 0x + 40 шестнадцатеричных знаков проверяется до отправки (окончательно — бэкенд);
    AddressList — адреса BSC через запятую, формат каждого проверяется так же;
    остальное — текст. Значения по умолчанию нет: поле показывает текущее значение или пусто.
  Кто вызывает: components/SettingCard.svelte, pages/Notifications.svelte, pages/Venues.svelte.
  От чего зависит: lib/settings.svelte.js, lib/labels.js, components/ErrorText.svelte.
  Настройки: любая — по описанию из схемы.
-->
<script>
  import { store, valueKey, setValue } from '../lib/settings.svelte.js';
  import { takePrefill } from '../lib/firstRun.svelte.js';
  import { showValue, choiceLabel } from '../lib/labels.js';
  import ErrorText from './ErrorText.svelte';

  /**
   * def — описание из схемы; scope — {kind, id?}; label — подпись области; options — варианты для TextList;
   * only — для Choice: какие из def.choices показать (например способы, которые есть у канала); null — все.
   */
  let { def, scope, label = '', options = null, only = null } = $props();

  const current = $derived(store.values[valueKey(def.name, scope)]);

  /**
   * Советуемое значение из «Первого запуска» — один раз при создании поля (как «не сохранено»), только если
   * настройка ещё не задана. Ничего не сохраняет: Тигран жмёт «Сохранить» обычным путём. Для Bool/TextList
   * ввод идёт не через draft — там совета нет, строки-переключатели в таблице первого запуска не предзаполняются.
   */
  function seedDraft() {
    const suggested = takePrefill(def.name, scope);
    const set = store.values[valueKey(def.name, scope)] !== undefined;
    return suggested !== undefined && !set ? suggested : null;
  }

  let draft = $state(seedDraft());
  let picked = $state(null);
  let error = $state('');
  let busy = $state(false);

  /** Текущее значение в виде текста для поля ввода. */
  const raw = $derived(
    current === undefined ? '' : Array.isArray(current) ? current.join(', ') : String(current),
  );
  const chosen = $derived(picked ?? (Array.isArray(current) ? current : []));

  // Формат адреса EVM (20 байт в hex) — задан самим протоколом, не настройка (docs/CODING_STANDARDS.md).
  const ADDRESS = /^0x[0-9a-fA-F]{40}$/;

  /**
   * Проверка до отправки — только то, что задано схемой (def.max) или форматом протокола (адрес).
   * Принимает: значение. Возвращает: причину по-русски или ''.
   */
  function precheck(value) {
    if (def.kind === 'Integer' && typeof value === 'number' && def.max != null && value > def.max) {
      return `Значение ${value} больше допустимого ${def.max}`;
    }
    if (def.kind === 'AddressList') {
      const bad = value.find((a) => !ADDRESS.test(a));
      if (bad !== undefined) return `«${bad}» — не адрес сети BSC (0x и 40 шестнадцатеричных знаков)`;
    }
    if (def.kind === 'Address' && !ADDRESS.test(value)) {
      return `«${value}» — не адрес сети BSC (0x и 40 шестнадцатеричных знаков)`;
    }
    return '';
  }

  /** Текст поля → значение для бэкенда по виду настройки. */
  function parse(text) {
    const t = text.trim();
    if (def.kind === 'Integer') return /^\d+$/.test(t) ? Number(t) : t;
    if (def.kind === 'TextList' || def.kind === 'AddressList') return t ? t.split(',').map((s) => s.trim()).filter(Boolean) : [];
    return t;
  }

  /** Сохраняет значение (null — очистить). Ошибка бэкенда показывается под полем. */
  async function save(value) {
    error = value === null ? '' : precheck(value);
    if (error) return;
    busy = true;
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
  <!-- Необязательная (def.optional) не задана — не «пробел», а выключенная часть функции (описание — ниже). -->
  <span class={current === undefined ? (def.optional ? 'muted' : 'unset') : 'value'}>
    {current === undefined ? (def.optional ? 'не задано (необязательная)' : 'не задано') : def.kind === 'Choice' ? choiceLabel(current) : showValue(current)}
  </span>
  {#if def.kind === 'Integer' && def.max != null}<span class="muted">не больше {def.max}</span>{/if}
  {#if def.kind === 'Bool'}
    <button disabled={busy || current === true} onclick={() => save(true)}>да</button>
    <button disabled={busy || current === false} onclick={() => save(false)}>нет</button>
  {:else if def.kind === 'Choice'}
    <select value={draft ?? current ?? ''} onchange={(e) => (draft = e.currentTarget.value)}>
      <option value="" disabled>— выбрать —</option>
      {#each def.choices.filter((c) => !only || only.includes(c)) as c (c)}<option value={c}>{choiceLabel(c)}</option>{/each}
    </select>
    <button disabled={busy || draft === null || draft === current} onclick={() => save(draft)}>Сохранить</button>
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
      placeholder={def.kind === 'TextList' ? 'через запятую' : def.kind === 'AddressList' ? '0x…, 0x… через запятую' : def.kind === 'Address' ? '0x…' : def.unit}
      class:addr={def.kind === 'Address' || def.kind === 'AddressList'}
      onkeydown={(e) => e.key === 'Enter' && draft !== null && save(parse(draft))}
    />
    <button disabled={busy || draft === null} onclick={() => save(parse(draft))}>Сохранить</button>
  {/if}
  {#if draft !== null || picked !== null}<span class="muted">не сохранено</span>{/if}
  <button class="quiet" disabled={busy || current === undefined} onclick={() => save(null)}>Очистить</button>
</div>
<ErrorText text={error} />
