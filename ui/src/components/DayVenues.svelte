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
  Решение 414: направление с заданными risk.drawdown_pct / risk.drawdown_max_usd (draw_on) — просадка от пика: слепок и
    как снят (чистый / по учёту, своё в пути и чужое на счёте), пик, теоретический баланс, просадка, предел (% и USDT,
    какой сработал), статус; «Сбросить лимит дня» — новый слепок (пик = слепок).
  Настройки: risk.drawdown_pct, risk.drawdown_max_usd, risk.max_daily_loss_usd (только показ; задаётся на экране «Настройки» → «Деньги и риск › по биржам и
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
    return all.filter(([, x]) => Number(x.result_usd) !== 0 || x.limit || x.paused || x.draw_on);
  });

  /** Решение 414: как снят слепок. */
  const way = (w) => (w === 'clean' ? 'чистый — идущих сделок направления не было' : 'по учёту — за 2 минуты сделки направления не кончились');
  /** Решение 414 (ревью t199, С3): чья пауза — просадки или старого лимита дня. */
  const why = (x) => (x.draw_paused ? 'по просадке от пика' : x.draw_on ? 'по старому лимиту дня (risk.max_daily_loss_usd, до перехода на просадку)' : 'по убытку дня');
  /** Решение 414: сделки слепка строкой. */
  const deals = (v) => (v ?? []).map(([id, a]) => `${id} ${a}`).join(', ');

  /** «Сбросить лимит дня» направления — только после подтверждения. Принимает: id пары, её итог. */
  function askReset(p, x) {
    if (x.draw_on) {
      const dp = x.paused ? '\nПауза по просадке снимется, когда слепок снят.' : '';
      const dq = `Сбросить лимит направления ${x.label}?\nБудет снят новый слепок баланса (сразу, как у направления не будет идущих сделок, но не дольше 2 минут — дальше по учёту сделок), пик = новый слепок. Другие направления, итог дня и сделки не меняются.${dp}`;
      if (confirm(dq)) resetVenueLimit(p);
      return;
    }
    const pause = x.paused ? '\nПауза этого направления по убытку дня будет снята — оно снова начнёт торговать.' : '';
    const q = `Сбросить лимит дня направления ${x.label}?\nДальше его лимит считает только убытки после этой отметки. Другие направления, итог дня, история и сделки не меняются.${pause}`;
    if (confirm(q)) resetVenueLimit(p);
  }
</script>

{#if only && !control.data}
  <p class="muted">Итог дня площадки неизвестен — нет данных ядра.</p>
{/if}
{#each rows as [p, x], pi (`${p}|${pi}`)}
  <div class={x.limit?.exceeded || x.draw?.exceeded || x.paused ? 'err' : 'muted'}>
    <code title={p}>{x.label}</code>: итог дня {x.result_usd} USDT (факт {x.final_usd}, оценка позиций {x.provisional_usd})
    {#if x.draw_on}
      {#if x.draw}
        <br />· просадка от пика (решение 414): слепок <strong>{x.draw.snap.base_usd}</strong> USDT
        <span class="muted">({way(x.draw.snap.way)}; {x.draw.snap.by}; {showTime(x.draw.snap.at)}; {x.draw.snap.place}: по факту {x.draw.snap.fact_usd}{#if x.draw.snap.own?.length} + своё в пути {x.draw.snap.own_usd} ({deals(x.draw.snap.own)}){/if}{#if x.draw.snap.foreign?.length} − чужое {x.draw.snap.foreign_usd} ({deals(x.draw.snap.foreign)}){/if})</span>
        · пик {x.draw.peak_usd} · теоретический баланс {x.draw.balance_usd}{#if Number(x.draw.pending_cost_usd) !== 0} (из него −{x.draw.pending_cost_usd} — плановые затраты возврата USDT){/if}
        · просадка <strong>{x.draw.drawdown_usd}</strong> из {x.draw.limit_usd} USDT
        <span class="muted">({x.draw.pct != null ? `${x.draw.pct} % от пика = ${x.draw.pct_usd}` : '% не задан'}; {x.draw.usd != null ? `не более ${x.draw.usd} USDT` : 'USDT не задан'} — сработал {x.draw.by === 'usd' ? 'USDT' : '%'})</span>{#if x.draw.exceeded} — превышена{/if}
      {:else}
        · лимит просадки: слепка баланса ещё нет — торговля этим направлением не включается
      {/if}
      {#if x.draw_want}
        <span class="muted"> · новый слепок снимается ({x.draw_want.by}, {showTime(x.draw_want.at)}){#if x.draw_want.note} — не снят: {x.draw_want.note}{/if}</span>
      {/if}
    {:else if x.limit}
      · лимит {x.limit.loss_usd} из {x.limit.max_usd} USDT{#if x.limit_reset} (сброшен в {showTime(x.limit_reset.at)}){/if}{#if x.limit.exceeded} — достигнут{/if}
    {:else}
      · лимит не задан (risk.max_daily_loss_usd) — торговля этим направлением не включается
    {/if}
    {#if x.paused}
      <strong> · {x.kind === 'dex_dex' ? `циклы DEX ↔ DEX на паузе ${why(x)} (биржи ↔ DEX торгуют)` : `направление на паузе ${why(x)} (другие направления торгуют)`}</strong>
      <span class="muted">({x.draw_paused ? 'снимется с новым слепком: новый день, UTC, или «Сбросить лимит»' : x.draw_on ? 'снимется «Сбросить лимит» — направление уже по просадке' : 'снимется сама с новым днём, UTC'})</span>
      <button class="quiet" disabled={control.busy} onclick={() => resumeVenue(p)}>Снять паузу {x.label}</button>
    {/if}
    <button class="quiet" disabled={control.busy} onclick={() => askReset(p, x)}>Сбросить лимит дня {x.label}</button>
  </div>
{/each}
