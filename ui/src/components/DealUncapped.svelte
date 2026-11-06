<!--
  Файл: ui/src/components/DealUncapped.svelte
  Что делает: в карточке сделки — расчёт «без потолка» (решение 141): «зашли на X USDT (что ограничило) → потенциал:
    вложить Y, прибыль Z; упущено Z − план; потенциал − факт». Данные — GET /api/uncapped/deal/{id} (404 — не посчитано:
    фича выключена, сделка до решения 141 или расчёт ещё идёт). Запись приходит от экрана «Сделки» одним запросом
    GET /api/uncapped/deals на все сделки (06.11.2026: запрос на каждую сделку — сотни 404 и лаги интерфейса); свой
    запрос — только по кнопке «Обновить».
  Кто вызывает: pages/Deals.svelte.
  От чего зависит: lib/api.js, lib/uncappedLabels.js, components/UncappedLine.svelte.
  Настройки: нет.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { limitedLabel, money } from '../lib/uncappedLabels.js';
  import UncappedLine from './UncappedLine.svelte';

  /** id — id сделки; fact — её итог (USDT) или null; initial — запись из GET /api/uncapped/deals (null — нет);
   *  loaded — список записей получен. */
  let { id, fact, initial = null, loaded = false } = $props();
  let own = $state(null);
  let why = $state('');
  const rec = $derived(own ?? initial);

  /** Читает запись сделки. Ошибка — в why. */
  async function load() {
    try {
      own = (await get(`/api/uncapped/deal/${seg(id)}`)).body;
      why = '';
    } catch (e) {
      own = null;
      why = e.message;
    }
  }
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
    Без потолка: {#if why}{why}{:else if loaded}для сделки {id} расчёта без потолка нет{:else}загрузка…{/if}
    <button type="button" onclick={load}>Обновить</button>
  </p>
{/if}
