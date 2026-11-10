<!--
  Файл: ui/src/components/CexCexDeny.svelte
  Что делает: решение 249 — список «не взаимодействовать» биржи (настройка venue.cex_cex_deny, область «площадка»)
    рядом с venue.cex_cex: текущий список и кнопка «Настроить список». Окно (components/Modal.svelte): добавленные биржи
    с кнопкой удаления у каждой, раскрывающийся список наших бирж из шлюза (store.venueKinds, вид Cex) — кроме самой
    биржи и уже добавленных; «Сохранить» — одной записью настройки (PUT /api/settings, журнал изменений как у всех
    настроек), «Отмена» и крестик — закрыть без сохранения (черновик теряется, при следующем открытии — сохранённый
    список). Ошибка сохранения — в окне. Запрет односторонний: сделка, начатая на этой бирже, не уводит монету и деньги
    на биржи из списка; при venue.cex_cex = нет список не важен.
  Кто вызывает: components/SettingCard.svelte и pages/Venues.svelte (у настройки venue.cex_cex, только у бирж).
  От чего зависит: lib/settings.svelte.js, lib/uniq.js, components/Modal.svelte, SettingHistory.svelte, ErrorText.svelte.
  Настройки: venue.cex_cex_deny (по площадке).
-->
<script>
  import { uniq } from '../lib/uniq.js';
  import { store, valueKey, setValue } from '../lib/settings.svelte.js';
  import Modal from './Modal.svelte';
  import SettingHistory from './SettingHistory.svelte';
  import ErrorText from './ErrorText.svelte';

  /** venue — id биржи, чей список настраивается. */
  let { venue } = $props();

  /** Имя настройки (решение 249, settings/catalog/trade.rs). */
  const NAME = 'venue.cex_cex_deny';
  const scope = $derived({ kind: 'Venue', id: venue });
  /** Сохранённый список (undefined — не задан). */
  const saved = $derived(store.values[valueKey(NAME, scope)]);
  const savedList = $derived(Array.isArray(saved) ? saved : []);

  let open = $state(false);
  let draft = $state([]);
  let pick = $state('');
  let error = $state('');
  let busy = $state(false);
  let showHistory = $state(false);

  /** Биржи шлюза, которые можно добавить: биржи (не DEX), кроме самой и уже добавленных. */
  const choices = $derived(
    uniq(store.venueKinds.filter((x) => x.kind === 'Cex').map((x) => x.id)).filter(
      (id) => id !== venue && !draft.includes(id),
    ),
  );
  const changed = $derived(JSON.stringify(draft) !== JSON.stringify(savedList));

  /** Открывает окно с сохранённым списком (несохранённое прошлое — забыто). */
  function start() {
    draft = [...savedList];
    pick = '';
    error = '';
    open = true;
  }
  /** Закрывает без сохранения («Отмена», крестик, Esc). */
  function close() {
    open = false;
  }
  /** Добавляет выбранную биржу в черновик. */
  function add() {
    if (pick && !draft.includes(pick)) draft = [...draft, pick];
    pick = '';
  }
  /** Убирает биржу из черновика. Принимает: id. */
  function remove(id) {
    draft = draft.filter((x) => x !== id);
  }
  /** Сохраняет черновик одной записью настройки; ошибка бэкенда — в окне, окно остаётся открытым. */
  async function save() {
    busy = true;
    error = '';
    try {
      await setValue(NAME, scope, draft);
      open = false;
    } catch (e) {
      error = e.message;
    }
    busy = false;
  }
</script>

<div class="deny">
  <span class="muted">Не взаимодействовать ({NAME}):</span>
  <span class:muted={!savedList.length}>
    {saved === undefined ? 'не задано — исключений нет' : savedList.length ? savedList.join(', ') : 'пусто — исключений нет'}
  </span>
  <button onclick={start}>Настроить список</button>
</div>

{#if open}
  <Modal title={`${venue}: не взаимодействовать с биржами`} onclose={close}>
    <p class="muted">
      Сделка, начатая на {venue} (покупка здесь), не уведёт монету и деньги на биржи из списка — ни продажей биржа →
      биржа, ни перегоном, в том числе через кошелёк DEX. Запрет односторонний: сделки, начатые на другой бирже, этот
      список не трогает. При venue.cex_cex = нет у {venue} список не важен — она не торгует с биржами в обе стороны.
    </p>
    <ul class="list">
      {#each uniq(draft) as id (id)}
        <li>
          <span>{id}</span>
          <button class="quiet" title="Убрать из списка" disabled={busy} onclick={() => remove(id)}>Удалить</button>
        </li>
      {:else}
        <li class="muted">Список пуст — исключений нет.</li>
      {/each}
    </ul>
    <div class="adder">
      <select bind:value={pick} disabled={busy || !choices.length}>
        <option value="">{choices.length ? '— выбрать биржу —' : 'больше нет бирж для добавления'}</option>
        {#each choices as id (id)}<option value={id}>{id}</option>{/each}
      </select>
      <button disabled={busy || !pick} onclick={add}>Добавить</button>
    </div>
    <ErrorText text={error} />
    <div class="actions">
      <button disabled={busy || !changed} onclick={save}>Сохранить</button>
      <button class="quiet" disabled={busy} onclick={close}>Отмена</button>
      {#if changed}<span class="muted">не сохранено</span>{/if}
    </div>
    <button class="quiet" onclick={() => (showHistory = !showHistory)}>
      {showHistory ? 'Скрыть историю' : 'История изменений'}
    </button>
    {#if showHistory}<SettingHistory name={NAME} scope={`venue:${venue}`} />{/if}
  </Modal>
{/if}

<style>
  .deny { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 4px 0 8px; }
  .list { list-style: none; padding: 0; margin: 8px 0; }
  .list li {
    display: flex; align-items: center; justify-content: space-between; gap: 8px;
    padding: 4px 0; border-bottom: 1px solid var(--line); max-width: 420px;
  }
  .adder, .actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 8px 0; }
</style>
