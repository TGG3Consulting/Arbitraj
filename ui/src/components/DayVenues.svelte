<!--
  Файл: ui/src/components/DayVenues.svelte
  Что делает: итог дня по направлениям — парам «площадка × тип направления» (решение 258, Тигран 12.11: «Каждое
    направление имеет свой лимит. Все.»; было — по площадке, решение 173): у каждой пары — факт, оценка позиций, лимит
    «X из Y USDT» (risk.max_daily_loss_usd: значение направления → площадки → вся система), пауза по убытку дня и кнопки
    «Снять паузу» (только этого направления) и «Сбросить лимит дня» этого направления с подтверждением (остальные
    направления не меняются). Сделка биржа → DEX и DEX → биржа считается бирже этого направления, биржа → биржа — бирже
    покупки («BingX → биржа»), DEX ↔ DEX — одно общее направление. Данные — Risk.day.pairs (GET /api/control).
  Кто вызывает: components/ControlBar.svelte (все направления), pages/Venues.svelte (одна площадка — only: её
    направления; у DEX — «DEX ↔ DEX»).
  От чего зависит: lib/control.svelte.js, lib/labels.js.
  Настройки: risk.max_daily_loss_usd (только показ; задаётся на экране «Настройки» → «Деньги и риск › по биржам и
    направлениям» или «Площадки»).
-->
<script>
  import { control, resetVenueLimit, resumeVenue } from '../lib/control.svelte.js';
  import { showTime } from '../lib/labels.js';

  /** only — id одной площадки (экран «Площадки»); не задан — все направления с итогом, лимитом или паузой. */
  let { only = null } = $props();

  const rows = $derived.by(() => {
    const all = Object.entries(control.data?.day?.pairs ?? {});
    if (only) {
      const own = all.filter(([, x]) => x.venue === only);
      // У DEX направлений биржи нет — его направление «DEX ↔ DEX» (одно на все циклы).
      return own.length ? own : all.filter(([, x]) => x.kind === 'dex_dex');
    }
    return all.filter(([, x]) => Number(x.result_usd) !== 0 || x.limit || x.paused);
  });

  /** «Сбросить лимит дня» направления — только после подтверждения. Принимает: id пары, её итог. */
  function askReset(p, x) {
    const pause = x.paused ? '\nПауза этого направления по убытку дня будет снята — оно снова начнёт торговать.' : '';
    const q = `Сбросить лимит дня направления ${x.label}?\nДальше его лимит считает только убытки после этой отметки. Другие направления, итог дня, история и сделки не меняются.${pause}`;
    if (confirm(q)) resetVenueLimit(p);
  }
</script>

{#if only && !control.data}
  <p class="muted">Итог дня площадки неизвестен — нет данных ядра.</p>
{/if}
{#each rows as [p, x], pi (`${p}|${pi}`)}
  <div class={x.limit?.exceeded || x.paused ? 'err' : 'muted'}>
    <code title={p}>{x.label}</code>: итог дня {x.result_usd} USDT (факт {x.final_usd}, оценка позиций {x.provisional_usd})
    {#if x.limit}
      · лимит {x.limit.loss_usd} из {x.limit.max_usd} USDT{#if x.limit_reset} (сброшен в {showTime(x.limit_reset.at)}){/if}{#if x.limit.exceeded} — достигнут{/if}
    {:else}
      · лимит не задан (risk.max_daily_loss_usd) — торговля этим направлением не включается
    {/if}
    {#if x.paused}
      <strong> · {x.kind === 'dex_dex' ? 'циклы DEX ↔ DEX на паузе по убытку дня (биржи ↔ DEX торгуют)' : 'направление на паузе по убытку дня (другие направления торгуют)'}</strong>
      <span class="muted">(снимется сама с новым днём, UTC)</span>
      <button class="quiet" disabled={control.busy} onclick={() => resumeVenue(p)}>Снять паузу {x.label}</button>
    {/if}
    <button class="quiet" disabled={control.busy} onclick={() => askReset(p, x)}>Сбросить лимит дня {x.label}</button>
  </div>
{/each}
