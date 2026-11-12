<!--
  Файл: ui/src/components/ControlBar.svelte
  Что делает: плашка режима системы на всех экранах (решение 46): «Стоп» (после запуска — до команды «Старт»),
    «В работе», «Пауза» и причина; кнопки «Старт», «Пауза», «Стоп»; паузы площадок и карантины монет
    с кнопками снятия; итог дня — всего, факт и оценка позиций, неразобранные оценки сделок (решения 53, 152: стоимость
    монет по рынку − вложено; «цены нет» — худший случай, вся сумма); лимит дня «X из Y» и кнопка «Сбросить лимит дня»
    с подтверждением (решение 153); остановлена по лимиту, а лимит уже не превышен — подсказка «можно нажать «Старт»».
    Решение 258 (было — по площадке, решение 173): итог дня по направлениям «площадка × тип» (факт / оценка / лимит /
    пауза, «Снять паузу» и «Сбросить лимит дня» направления — DayVenues.svelte); пауза по убытку дня — только этого
    направления (Risk.pair_paused), паузы площадки — отдельно (venue_paused). Общий лимит — «Стоп» всей системы
    (risk.max_daily_loss_total_usd), его «Сбросить лимит дня» сбрасывает всю систему и все направления.
    Решение 225: у паузы — кто поставил: поставленную ботом (venue_auto, mode_auto, пауза по убытку дня) бот снимает сам —
    подпись «снимется сама …»; «исход неизвестен» и паузу человека снимает только человек.
    Решение 253, дополнение t85: пауза «лимит вывода исчерпан» (WithdrawLimit) — снимает только человек после сброса
    лимита (время сброса биржа не сообщает).
    Решение 230: паузы кошельков DEX от сверки (wallet_paused) — строка кошелька с кнопкой «Снять паузу»
    (POST /api/control/wallet/{id}/resume); снимается и сама, когда балансы кошелька сойдутся.
    Команды уходят ядру (POST /api/control…) и пишутся в журнал.
  Кто вызывает: App.svelte.
  От чего зависит: lib/control.svelte.js, lib/coreLabels.js, lib/labels.js, components/DayVenues.svelte.
  Настройки: risk.max_daily_loss_usd, risk.max_daily_loss_total_usd (только показ; задаются на экране «Настройки»).
-->
<script>
  import { control, setMode, resumeVenue, resumeWallet, releaseCoin, resetDailyLimit } from '../lib/control.svelte.js';
  import { modeLabel } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import DayVenues from './DayVenues.svelte';

  const r = $derived(control.data);
  const cls = $derived(r?.mode === 'Running' ? 'run' : r?.mode === 'Paused' ? 'warn' : 'stop');
  const lim = $derived(r?.day?.limit);
  const reset = $derived(r?.day?.limit_reset);
  // Убыток для лимита до сброса: −итог дня в момент сброса (отрицательное — день был в плюсе).
  const before = $derived(reset ? -Number(reset.base_usd) : 0);

  /** Решение 225: подпись софтовой паузы площадки по виду отметки (Errors / Rule / WithdrawLimit / Outcome). */
  function autoNote(kind) {
    if (kind === 'Outcome') return 'исход неизвестен — снимает только человек';
    if (kind === 'WithdrawLimit') return 'лимит вывода исчерпан — снимите паузу после сброса лимита (время сброса биржа не сообщает)';
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
    const q = `Сбросить лимит дня — вся система и все направления площадок?\nДальше лимиты считают только убытки после этой отметки; паузы направлений по убытку дня снимаются. Итог дня, история и сделки не меняются.${stopNote}`;
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
        Общий лимит дня не задан (risk.max_daily_loss_total_usd) — общего стопа по убытку нет, действуют лимиты направлений
      {/if}
      <button class="quiet" disabled={control.busy} onclick={askReset}>Сбросить лимит дня (все)</button>
    </div>
    <DayVenues />
    {#if r.daily_stop && lim && !lim.exceeded}
      <div><strong>Лимит сейчас не превышен:</strong> {lim.loss_usd} из {lim.max_usd} USDT — остановка по лимиту снимется сама с новым днём (UTC), можно нажать «Старт».</div>
    {/if}
    {#each Object.entries(r.venue_paused) as [v, why] (v)}
      <div>Площадка <code>{v}</code> на паузе: {why} — все её направления{#if r.venue_auto?.[v]} <span class="muted">({autoNote(r.venue_auto[v].kind)})</span>{/if} <button class="quiet" onclick={() => resumeVenue(v)}>Снять паузу</button></div>
    {/each}
    {#each Object.entries(r.wallet_paused ?? {}) as [id, w] (id)}
      <div>Кошелёк «{w.name}» (<code>{w.venue}</code>, {id}) на паузе: {w.why} — новые сделки этим кошельком не начинаются, остальные кошельки торгуют <span class="muted">(поставила сверка: снимется сама, когда балансы сойдутся — ledger.reconcile_confirm_rounds чистых кругов подряд)</span> <button class="quiet" onclick={() => resumeWallet(id)}>Снять паузу</button></div>
    {/each}
    {#each Object.entries(r.day.provisional) as [deal, [usd, day]] (deal)}
      <div class="muted">Оценка позиции сделки <code>{deal}</code>{#if r.day.deal_pairs?.[deal]} ({r.day.pairs?.[r.day.deal_pairs[deal]]?.label ?? r.day.deal_pairs[deal]}){/if}: {usd} USDT (учтена {day}){#if r.day.unpriced?.includes(deal)} — <span class="err">цены нет, взят худший случай (вся сумма)</span>{/if} — ждёт разбора</div>
    {/each}
    {#each r.quarantine as q, qi (`${q.id}|${qi}`)}
      <div>Монета <code title={q.id}>{q.label}</code> в карантине: {q.reason} <button class="quiet" onclick={() => releaseCoin(q.id)}>Снять карантин</button></div>
    {/each}
  {:else}
    <span class="muted">Режим системы неизвестен{control.error ? `: ${control.error}` : ' — загружаю…'}</span>
  {/if}
  {#if r && control.error}<span class="err">{control.error}</span>{/if}
</div>
