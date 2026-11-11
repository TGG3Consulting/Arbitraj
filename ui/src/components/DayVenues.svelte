<!--
  Файл: ui/src/components/DayVenues.svelte
  Что делает: итог дня по площадкам (решение 173): у каждой площадки — факт, оценка позиций, лимит «X из Y USDT»
    (risk.max_daily_loss_usd: значение площадки или «вся система»), пауза по убытку дня и кнопка «Сбросить лимит дня»
    площадки с подтверждением (снимает её паузу по убытку; остальные площадки не меняются). Сделка CEX ↔ DEX
    считается бирже, CEX ↔ CEX — бирже покупки (решение 173, дополнение t91), DEX ↔ DEX — DEX. Данные — Risk.day.venues (GET /api/control).
  Кто вызывает: components/ControlBar.svelte (все площадки), pages/Venues.svelte (одна площадка — only).
  От чего зависит: lib/control.svelte.js, lib/labels.js.
  Настройки: risk.max_daily_loss_usd (только показ; задаётся на экране «Настройки» или «Площадки»).
-->
<script>
  import { control, resetVenueLimit } from '../lib/control.svelte.js';
  import { showTime } from '../lib/labels.js';

  /** only — id одной площадки (экран «Площадки»); не задан — все площадки с итогом, лимитом или паузой. */
  let { only = null } = $props();

  const rows = $derived.by(() => {
    const all = Object.entries(control.data?.day?.venues ?? {});
    if (only) return all.filter(([v]) => v === only);
    return all.filter(([, x]) => Number(x.result_usd) !== 0 || x.limit || x.paused);
  });

  /** «Сбросить лимит дня» площадки — только после подтверждения. Принимает: id площадки, её итог. */
  function askReset(v, x) {
    const pause = x.paused ? '\nПауза площадки по убытку дня будет снята — она снова начнёт торговать.' : '';
    const q = `Сбросить лимит дня площадки ${v}?\nДальше её лимит считает только убытки после этой отметки. Итог дня, история и сделки не меняются.${pause}`;
    if (confirm(q)) resetVenueLimit(v);
  }
</script>

{#if only && !control.data}
  <p class="muted">Итог дня площадки неизвестен — нет данных ядра.</p>
{/if}
{#each rows as [v, x], vi (`${v}|${vi}`)}
  <div class={x.limit?.exceeded || x.paused ? 'err' : 'muted'}>
    <code>{v}</code>: итог дня {x.result_usd} USDT (факт {x.final_usd}, оценка позиций {x.provisional_usd})
    {#if x.limit}
      · лимит {x.limit.loss_usd} из {x.limit.max_usd} USDT{#if x.limit_reset} (сброшен в {showTime(x.limit_reset.at)}){/if}{#if x.limit.exceeded} — достигнут{/if}
    {:else}
      · лимит не задан (risk.max_daily_loss_usd) — торговля с этой площадкой не включается
    {/if}
    {#if x.paused}<strong> · {control.data?.cycle_paused?.[v] ? 'циклы DEX ↔ DEX на паузе по убытку дня (биржи ↔ DEX торгуют)' : 'на паузе по убытку дня'}</strong>{/if}
    <button class="quiet" disabled={control.busy} onclick={() => askReset(v, x)}>Сбросить лимит дня {v}</button>
  </div>
{/each}
