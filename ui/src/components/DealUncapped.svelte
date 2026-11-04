<!--
  Файл: ui/src/components/DealUncapped.svelte
  Что делает: в карточке сделки — расчёт «без потолка» (решение 141): «зашли на X USDT (что ограничило) → потенциал:
    вложить Y, прибыль Z; упущено Z − план; потенциал − факт». Данные — GET /api/uncapped/deal/{id} (404 — не посчитано:
    фича выключена, сделка до решения 141 или расчёт ещё идёт).
  Кто вызывает: pages/Deals.svelte.
  От чего зависит: lib/api.js, lib/uncappedLabels.js, components/UncappedLine.svelte.
  Настройки: нет.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { limitedLabel, money } from '../lib/uncappedLabels.js';
  import UncappedLine from './UncappedLine.svelte';

  /** id — id сделки; fact — её итог (USDT) или null. */
  let { id, fact } = $props();
  let rec = $state(null);
  let why = $state('');

  /** Читает запись сделки. Ошибка — в why. */
  async function load() {
    try {
      rec = (await get(`/api/uncapped/deal/${seg(id)}`)).body;
      why = '';
    } catch (e) {
      rec = null;
      why = e.message;
    }
  }

  $effect(() => {
    void id;
    load();
  });
</script>

{#if rec}
  <p>
    Зашли на <strong>{money(rec.battle.invested)} USDT</strong> (ограничило: {limitedLabel(rec.limited_by)}) → потенциал:
    вложить <strong>{money(rec.opt.invested)} USDT</strong>, прибыль <strong>{money(rec.opt.profit)} USDT</strong>;
    план {money(rec.battle.profit)}{#if fact != null}, факт {money(fact)}, потенциал − факт
      <strong>{money(Number(rec.opt_passes ? rec.opt.profit : 0) - Number(fact))} USDT</strong>{/if}
  </p>
  <UncappedLine u={rec} />
{:else}
  <p class="muted">
    Без потолка: {#if why}{why}{:else}загрузка…{/if}
    <button type="button" onclick={load}>Обновить</button>
  </p>
{/if}
