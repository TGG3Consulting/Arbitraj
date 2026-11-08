<!--
  Файл: ui/src/components/RuleForm.svelte
  Что делает: форма правила шлюза — событие (код ошибки), область (общее или площадка), действие
    и его параметры, необязательное условие (JSON), включено ли. Списки — из GET /api/rules/meta
    и GET /api/venues. Значений по умолчанию нет: всё выбирается и вводится вручную.
    Создание — POST /api/rules, изменение — PUT /api/rules/{id}. Ошибка бэкенда (например, дубль) — как есть.
    Для кода с no_retry («результат неизвестен», решение 64) «Повторить» в списке действий нет. Решение 230: для отказа
    вывода (WithdrawRejected, InvalidAddress, WithdrawLocked) нет «Повторить», «Стоп площадка», «Стоп всё» (meta
    codes[].inactive) — подпись «только предупреждение»; у старого правила с таким действием — «не действует: …».
  Кто вызывает: pages/Rules.svelte.
  От чего зависит: lib/api.js, lib/settings.svelte.js (площадки), components/ErrorText.svelte.
  Настройки: нет (правила — данные из интерфейса, src/gateway/rules.rs).
-->
<script>
  import { uniq } from '../lib/uniq.js';
  import { untrack } from 'svelte';
  import { post, put } from '../lib/api.js';
  import { store } from '../lib/settings.svelte.js';
  import ErrorText from './ErrorText.svelte';

  /** meta — справочник; rule — правило для изменения или null (новое); done — вызвать после сохранения. */
  let { meta, rule = null, done } = $props();

  /** Вид действия правила: строка или ключ объекта {"Retry": {...}}. */
  const actionKind = (a) => (typeof a === 'string' ? a : Object.keys(a)[0]);

  // Форма заполняется один раз из правила (RuleForm пересоздаётся при выборе другого правила).
  const r0 = untrack(() => rule);
  let event = $state(r0?.event ?? '');
  let scopeKind = $state(r0?.scope.kind ?? '');
  let venue = $state(r0?.scope.venue ?? '');
  let action = $state(r0 ? actionKind(r0.action) : '');
  let params = $state(r0 && typeof r0.action === 'object' ? { ...Object.values(r0.action)[0] } : {});
  let condition = $state(r0?.condition ? JSON.stringify(r0.condition) : '');
  // Новое правило: включено ли — выбирается явно (значения по умолчанию нет).
  let enabled = $state(r0 ? String(r0.enabled) : '');
  let error = $state('');
  let busy = $state(false);

  const actionMeta = $derived(meta.actions.find((a) => a.kind === action));
  /** Решения 64, 230: действия, которые для кода не действуют (вид → причина): для «результат неизвестен» —
   *  «Повторить»; для отказа вывода (WithdrawRejected, InvalidAddress, WithdrawLocked) — «Повторить», «Стоп площадка»,
   *  «Стоп всё» (повтора нет, паузы нет — только предупреждение). Их в списке нет. */
  const inactive = $derived(meta.codes.find((c) => c.code === event)?.inactive ?? {});
  const actions = $derived(meta.actions.filter((a) => !inactive[a.kind]));
  /** Для кода отказа вывода «стоп» не действует — честная подпись: только предупреждение. */
  const warnOnly = $derived(Boolean(inactive.StopVenue || inactive.StopAll));

  /** Собирает правило в формате бэкенда. Ошибки: Error с причиной (не выбрано, не число, не JSON). */
  function build() {
    if (!event || !scopeKind || !action || !enabled) {
      throw new Error('Выберите событие, область, действие и состояние');
    }
    if (scopeKind === 'Venue' && !venue) throw new Error('Выберите площадку');
    if (inactive[action] && enabled === 'true') throw new Error(`Это действие для события не действует: ${inactive[action]}`);
    let act = action;
    if (actionMeta.params.length) {
      const p = {};
      for (const m of actionMeta.params) {
        const t = String(params[m.name] ?? '').trim();
        if (!/^\d+$/.test(t)) throw new Error(`«${m.title}» — нужно целое число`);
        p[m.name] = Number(t);
      }
      act = { [action]: p };
    }
    let cond = null;
    if (condition.trim()) {
      try {
        cond = JSON.parse(condition);
      } catch (e) {
        throw new Error(`Условие — не JSON: ${e.message}`);
      }
    }
    const scope = scopeKind === 'Venue' ? { kind: 'Venue', venue } : { kind: scopeKind };
    // id нового правила бэкенд назначает сам (поле в запросе игнорирует).
    return { id: rule ? rule.id : 0, event, condition: cond, action: act, scope, enabled: enabled === 'true' };
  }

  /** Сохраняет правило; при успехе вызывает done. */
  async function save() {
    busy = true;
    error = '';
    try {
      const r = build();
      if (rule) await put(`/api/rules/${rule.id}`, r);
      else await post('/api/rules', r);
      done();
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<form class="card" onsubmit={(e) => { e.preventDefault(); save(); }}>
  <h2>{rule ? `Изменить правило №${rule.id}` : 'Новое правило'}</h2>
  <label>Событие (код ошибки)
    <select bind:value={event}>
      <option value="">— выберите —</option>
      {#each meta.codes as c (c.code)}<option value={c.code}>{c.code} — {c.title}</option>{/each}
    </select>
  </label>
  <label>Область
    <select bind:value={scopeKind}>
      <option value="">— выберите —</option>
      {#each meta.scopes as s (s.kind)}<option value={s.kind}>{s.title}</option>{/each}
    </select>
  </label>
  {#if scopeKind === 'Venue'}
    <label>Площадка
      <select bind:value={venue}>
        <option value="">— выберите —</option>
        {#each uniq(store.venues) as v (v)}<option value={v}>{v}</option>{/each}
      </select>
    </label>
  {/if}
  <label>Действие
    <select bind:value={action}>
      <option value="">— выберите —</option>
      {#each actions as a (a.kind)}<option value={a.kind}>{a.title}</option>{/each}
      {#if inactive[action]}<option value={action} disabled>{meta.actions.find((a) => a.kind === action)?.title ?? action} — не действует</option>{/if}
    </select>
  </label>
  {#if inactive[action]}<p class="err">Сохранённое действие не действует: {inactive[action]}. Выберите другое (или выключите правило).</p>{/if}
  {#if warnOnly}<p class="muted">Для этого события — только предупреждение: повтора и паузы нет ({inactive.StopVenue ?? inactive.StopAll}).</p>{/if}
  {#each actionMeta?.params ?? [] as p (p.name)}
    <label>{p.title}, {p.unit}<input class="num" inputmode="numeric" bind:value={params[p.name]} /></label>
  {/each}
  <label>Условие (необязательно, JSON)
    <input bind:value={condition} placeholder="пусто — без условия" />
  </label>
  <p class="muted">Правило с условием пока не применяется: формат условия утверждается на этапе 5.</p>
  <label>Состояние
    <select bind:value={enabled}>
      <option value="">— выберите —</option>
      <option value="true">включено</option>
      <option value="false">выключено</option>
    </select>
  </label>
  <div class="row">
    <button disabled={busy}>{rule ? 'Сохранить' : 'Создать'}</button>
    <button type="button" class="quiet" onclick={done}>Отмена</button>
  </div>
  <ErrorText text={error} />
</form>
