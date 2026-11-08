<!--
  Файл: ui/src/components/ControlBar.svelte
  Что делает: плашка режима системы на всех экранах (решение 46): «Стоп» (после запуска — до команды «Старт»),
    «В работе», «Пауза» и причина; кнопки «Старт», «Пауза», «Стоп»; паузы площадок и карантины монет
    с кнопками снятия; итог дня — всего, факт и оценка позиций, неразобранные оценки сделок (решения 53, 152: стоимость
    монет по рынку − вложено; «цены нет» — худший случай, вся сумма); лимит дня «X из Y» и кнопка «Сбросить лимит дня»
    с подтверждением (решение 153); остановлена по лимиту, а лимит уже не превышен — подсказка «можно нажать «Старт»».
    Решение 173: итог дня по площадкам (факт / оценка / лимит / пауза, «Сбросить лимит дня» площадки —
    DayVenues.svelte); общий лимит — «Стоп» всей системы (risk.max_daily_loss_total_usd), его «Сбросить лимит дня»
    сбрасывает всю систему и все площадки.
    Решение 225: у паузы — кто поставил: поставленную ботом (venue_auto, mode_auto, пауза по убытку дня) бот снимает сам —
    подпись «снимется сама …»; «исход неизвестен» и паузу человека снимает только человек.
    Команды уходят ядру (POST /api/control…) и пишутся в журнал.
  Кто вызывает: App.svelte.
  От чего зависит: lib/control.svelte.js, lib/coreLabels.js, lib/labels.js, components/DayVenues.svelte.
  Настройки: risk.max_daily_loss_usd, risk.max_daily_loss_total_usd (только показ; задаются на экране «Настройки»).
-->
<script>
  import { control, setMode, resumeVenue, releaseCoin, resetDailyLimit } from '../lib/control.svelte.js';
  import { modeLabel } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import DayVenues from './DayVenues.svelte';

  const r = $derived(control.data);
  const cls = $derived(r?.mode === 'Running' ? 'run' : r?.mode === 'Paused' ? 'warn' : 'stop');
  const lim = $derived(r?.day?.limit);
  const reset = $derived(r?.day?.limit_reset);
  // Убыток для лимита до сброса: −итог дня в момент сброса (отрицательное — день был в плюсе).
  const before = $derived(reset ? -Number(reset.base_usd) : 0);

  /** Решение 225: подпись софтовой паузы площадки по виду отметки (Errors / Rule / Outcome). */
  function autoNote(kind) {
    if (kind === 'Outcome') return 'исход неизвестен — снимает только человек';
    if (kind === 'Errors') return 'поставил бот: снимется сама, когда проверка подключения пройдёт (через risk.errors_resume_after_s, дальше каждые risk.auto_resume_check_s)';
    return 'поставил бот: снимется сама, когда проверка подключения пройдёт (каждые risk.auto_resume_check_s)';
  }

  /** Решение 225: подпись софтового «Стоп» системы (DailyTotal / Rule). */
  function modeNote(m) {
    if (m.kind === 'DailyTotal') return 'поставил бот: снимется сам с новым днём (UTC)';
    const v = m.kind?.Rule?.venue ?? '';
    return `поставил бот: снимется сам, когда проверка подключения ${v} пройдёт (каждые risk.auto_resume_check_s)`;
  }

  /** «Сбросить лимит дня» — только после подтверждения. */
  function askReset() {
    const stopNote = r?.daily_stop ? '\nСистема остановлена по лимиту — после сброса она останется в «Стоп», запуск — кнопкой «Старт».' : '';
    const q = `Сбросить лимит дня — вся система и все площадки?\nДальше лимиты считают только убытки после этой отметки; паузы площадок по убытку дня снимаются. Итог дня, история и сделки не меняются.${stopNote}`;
    if (confirm(q)) resetDailyLimit();
  }
</script>

<div class="banner control {cls}" role="status">
  {#if r}
    <strong>Режим: {modeLabel(r.mode)}</strong>
    {#if r.reason}<span>— {r.reason}</span>{/if}
    {#if r.mode_auto && r.mode === 'Stopped'}<span class="muted">({modeNote(r.mode_auto)})</span>{/if}
    <span class="muted" title="Оценка позиций — остановленные сделки и «Беру на себя»: стоимость монет по рынку площадки, где они лежат, минус вложено (решение 152); цены нет — худший случай (вся сумма). После разбора заменяется фактом">
      · итог дня {r.day.date}: {r.day.result_usd} USDT (факт {r.day.final_usd}, оценка позиций {r.day.provisional_usd})
    </span>
    <span class="row inline">
      <button disabled={control.busy || r.mode === 'Running'} onclick={() => setMode('Running')}>Старт</button>
      <button class="quiet" disabled={control.busy || r.mode !== 'Running'} onclick={() => setMode('Paused')}>Пауза</button>
      <button class="quiet" disabled={control.busy || r.mode === 'Stopped'} onclick={() => setMode('Stopped')}>Стоп</button>
    </span>
    <div class={lim?.exceeded ? 'err' : 'muted'}>
      {#if lim}
        Общий лимит дня (Стоп всей системы): {lim.loss_usd} из {lim.max_usd} USDT{#if reset} (сброшен в {showTime(reset.at)}, до сброса {before >= 0 ? `проиграно ${before}` : `итог +${-before}`} USDT){/if}{#if lim.exceeded} — достигнут{/if}
      {:else}
        Общий лимит дня не задан (risk.max_daily_loss_total_usd) — общего стопа по убытку нет, действуют лимиты площадок
      {/if}
      <button class="quiet" disabled={control.busy} onclick={askReset}>Сбросить лимит дня (все)</button>
    </div>
    <DayVenues />
    {#if r.daily_stop && lim && !lim.exceeded}
      <div><strong>Лимит сейчас не превышен:</strong> {lim.loss_usd} из {lim.max_usd} USDT — остановка по лимиту снимется сама с новым днём (UTC), можно нажать «Старт».</div>
    {/if}
    {#each Object.entries(r.cycle_paused ?? {}) as [v, why] (v)}
      <div>Циклы DEX ↔ DEX (<code>{v}</code>) на паузе: {why} — направления бирж с DEX торгуют <span class="muted">(снимется сама с новым днём, UTC)</span> <button class="quiet" onclick={() => resumeVenue(v)}>Снять паузу</button></div>
    {/each}
    {#each Object.entries(r.venue_paused) as [v, why] (v)}
      <div>Площадка <code>{v}</code> на паузе: {why}{#if r.venue_auto?.[v]} <span class="muted">({autoNote(r.venue_auto[v].kind)})</span>{:else if r.day?.venues?.[v]?.paused} <span class="muted">(снимется сама с новым днём, UTC)</span>{/if} <button class="quiet" onclick={() => resumeVenue(v)}>Снять паузу</button></div>
    {/each}
    {#each Object.entries(r.day.provisional) as [deal, [usd, day]] (deal)}
      <div class="muted">Оценка позиции сделки <code>{deal}</code>{#if r.day.deal_venues?.[deal]} ({r.day.deal_venues[deal].join(', ')}){/if}: {usd} USDT (учтена {day}){#if r.day.unpriced?.includes(deal)} — <span class="err">цены нет, взят худший случай (вся сумма)</span>{/if} — ждёт разбора</div>
    {/each}
    {#each r.quarantine as q, qi (`${q.id}|${qi}`)}
      <div>Монета <code title={q.id}>{q.label}</code> в карантине: {q.reason} <button class="quiet" onclick={() => releaseCoin(q.id)}>Снять карантин</button></div>
    {/each}
  {:else}
    <span class="muted">Режим системы неизвестен{control.error ? `: ${control.error}` : ' — загружаю…'}</span>
  {/if}
  {#if r && control.error}<span class="err">{control.error}</span>{/if}
</div>
