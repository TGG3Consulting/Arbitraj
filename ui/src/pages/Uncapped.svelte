<!--
  Файл: ui/src/pages/Uncapped.svelte
  Что делает: экран «Упущено из-за потолка» (решение 141) за период (сутки / 7 дней / свой): два итога — «по
    исполненным сделкам» (план, факт, та же модель на нашем бюджете, потенциал без потолка, упущено, сколько упёрлось в
    бюджет) и «по не исполненным кандидатам» (эпизоды: упущено — пиковое, сколько отклонённых стали бы плюсовыми);
    таблица «потолок → прибыль» (значения — uncapped.ladder_usdt) и строка «без потолка»; топ направлений; счётчики
    фоновой очереди (посчитано, слито, не посчитано: очередь полна, сбои и последняя причина, мс на расчёт); записи
    периода страницей (сервер), по строке — подробности: состав затрат на оптимуме без потолка, минимум до плюса,
    таблица потолков записи, эпизод. Решение 197: окно — измеритель упущенной прибыли: сделки — все (зашли на X, почему
    не больше, без потолка вложить Y → прибыль Z, упущено, факт из учёта, окно от спреда выше порога до входа);
    кандидаты — только с упущено > 0 (по умолчанию; старые записи с упущено 0 скрыты, итог их не считает), окно —
    первый → последний расчёт с упущенным. Слова «бой» нет: «на нашем бюджете».
    Данные — GET /api/uncapped/summary, GET /api/uncapped, GET /api/uncapped/{id}.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/coinNames.svelte.js, lib/labels.js, lib/uncappedLabels.js, components/PageSize.svelte, ErrorText.svelte,
    UncappedRecord.svelte.
  Настройки: нет (uncapped.* — экран «Настройки», категория «Упущено из-за потолка»). Период, топ и строки на странице
    задаёт зритель.
-->
<script>
  import { uniqBy } from '../lib/uniq.js';
  import { get } from '../lib/api.js';
  import { coinName } from '../lib/coinNames.svelte.js';
  import { showTime } from '../lib/labels.js';
  import { limitedLabel, budgetBound, money, period, verdictLabel, duration, windowSec } from '../lib/uncappedLabels.js';
  import PageSize from '../components/PageSize.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import UncappedRecord from '../components/UncappedRecord.svelte';

  let kind = $state('day');
  let fromLocal = $state('');
  let toLocal = $state('');
  let top = $state(null);
  let limit = $state(null);
  let only = $state('');
  /** Решение 197: только записи с упущенным (сделки — все). */
  let missedOnly = $state(true);
  let page = $state(1);
  let sum = $state(null);
  let rows = $state([]);
  let total = $state(0);
  let open = $state(null);
  let error = $state('');
  const pages = $derived(limit ? Math.max(1, Math.ceil(total / limit)) : 1);

  /** Перечитывает итоги и страницу. Ошибка — в error. */
  async function reload() {
    const p = period(kind, fromLocal, toLocal);
    const q = new URLSearchParams();
    if (p.from) q.set('from', p.from);
    if (p.to) q.set('to', p.to);
    try {
      if (top) sum = await get(`/api/uncapped/summary?${q}&top=${top}`);
      if (limit) {
        const l = new URLSearchParams(q);
        l.set('limit', limit);
        l.set('offset', (page - 1) * limit);
        if (only) l.set('kind', only);
        if (missedOnly) l.set('missed_only', 'true');
        const r = await get(`/api/uncapped?${l}`);
        rows = r.rows;
        total = r.total;
      }
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    void [kind, fromLocal, toLocal, only, limit, missedOnly];
    page = 1;
  });
  $effect(() => {
    void [kind, fromLocal, toLocal, only, limit, top, page, missedOnly];
    reload();
  });
  const dirText = (d) => `${coinName(d.asset)} ${d.buy_on} → ${d.sell_on}`;
</script>

<h1>Упущено из-за потолка</h1>
<ErrorText text={error} />
<div class="row">
  <label class="inline">Период
    <select bind:value={kind}>
      <option value="day">сутки</option>
      <option value="week">7 дней</option>
      <option value="custom">свой</option>
    </select>
  </label>
  {#if kind === 'custom'}
    <label class="inline">с <input type="datetime-local" bind:value={fromLocal} /></label>
    <label class="inline">по <input type="datetime-local" bind:value={toLocal} /></label>
  {/if}
  <PageSize key="uncapped.top" bind:value={top} label="Топ направлений" />
  <button type="button" onclick={reload}>Обновить</button>
</div>
{#if !top}<p class="muted">Задайте «Топ направлений», чтобы увидеть итоги.</p>{/if}
{#if sum}
  {#if sum.off}<p class="warn-text">Расчёт без потолка не идёт: {sum.off}</p>{/if}
  <table>
    <thead><tr><th></th><th>записей</th><th>упёрлись в бюджет</th><th>план на нашем бюджете</th><th>факт</th><th>та же модель на нашем бюджете</th><th>без потолка</th><th>упущено</th><th>окна</th></tr></thead>
    <tbody>
      <tr><td>Исполненные сделки</td><td>{sum.deals.count}</td><td>{sum.deals.budget_bound}</td><td>{money(sum.deals.battle_profit)}</td>
        <td>{money(sum.deals.fact)} <span class="muted">({sum.deals.fact_count} с итогом)</span></td><td>{money(sum.deals.at_budget_profit)}</td>
        <td><strong>{money(sum.deals.potential_profit)}</strong></td><td><strong>{money(sum.deals.missed)}</strong></td>
        <td>{sum.deals.windows}: {duration(sum.deals.window_s)}</td></tr>
      <tr><td>Не исполненные: стали бы плюсовыми или больше без потолка (эпизоды)</td><td>{sum.candidates.count}</td><td>{sum.candidates.budget_bound}</td><td>{money(sum.candidates.battle_profit)}</td>
        <td class="muted">—</td><td>{money(sum.candidates.at_budget_profit)}</td>
        <td><strong>{money(sum.candidates.potential_profit)}</strong></td><td><strong>{money(sum.candidates.missed)}</strong>
          <span class="muted">(отклонённых, плюсовых без потолка: {sum.candidates.plus_uncapped})</span></td>
        <td>{sum.candidates.windows}: {duration(sum.candidates.window_s)}</td></tr>
    </tbody>
  </table>
  {#if sum.hidden_zero}<p class="muted">Старых записей кандидатов с упущено 0 (до решения 197) в итог не входит: {sum.hidden_zero}.</p>{/if}
  <h2>Потолок → прибыль («взяли бы»: лучший объём со списанием не больше потолка, если проходит пороги)</h2>
  {#if sum.ladder_error}<p class="warn-text">{sum.ladder_error}</p>{/if}
  <table>
    <thead><tr><th>потолок, USDT</th><th>сделки</th><th>кандидаты</th></tr></thead>
    <tbody>
      {#each sum.ladder as l, li (`${l.cap}|${li}`)}<tr><td>{l.cap}</td><td>{money(l.deals)}</td><td>{money(l.candidates)}</td></tr>{/each}
      <tr><td><strong>без потолка</strong></td><td><strong>{money(sum.uncapped.deals)}</strong></td><td><strong>{money(sum.uncapped.candidates)}</strong></td></tr>
    </tbody>
  </table>
  <h2>Топ направлений по упущенному</h2>
  <table><tbody>
    {#each sum.top as t, ti (`${t.direction}|${ti}`)}<tr><td>{t.direction}</td><td>{money(t.missed)} USDT</td><td class="muted">записей {t.count}</td></tr>{/each}
  </tbody></table>
  <p class="muted">
    Очередь: посчитано {sum.stats.counters.done} (кандидатов без упущенного, не записаны: {sum.stats.counters.no_missed}),
    слито повторов {sum.stats.counters.merged}, сделок ждут {sum.stats.counters.deals_waiting} (сделки — вне очереди кандидатов),
    <strong>не посчитано (очередь полна) {sum.stats.counters.skipped_full}</strong>, сбоев {sum.stats.counters.failed},
    в работе {sum.stats.counters.running}, ждут {sum.stats.counters.waiting}; расчёт в среднем {money(sum.stats.mean_ms)} мс,
    наибольший {money(sum.stats.counters.max_ms)} мс
    {#if sum.stats.counters.last_error}<br /><span class="err">последний сбой: {sum.stats.counters.last_error}</span>{/if}
  </p>
{/if}

<h2>Записи периода</h2>
<div class="row">
  <label class="inline">Вид
    <select bind:value={only}><option value="">все</option><option value="deal">сделки</option><option value="candidate">кандидаты</option></select>
  </label>
  <label class="inline"><input type="checkbox" bind:checked={missedOnly} /> только с упущенным (сделки — все)</label>
  <PageSize key="uncapped.rows" bind:value={limit} label="Строк на странице" />
  {#if limit}
    <button type="button" disabled={page <= 1} onclick={() => (page -= 1)}>←</button>
    <span>{page} / {pages} (всего {total})</span>
    <button type="button" disabled={page >= pages} onclick={() => (page += 1)}>→</button>
  {/if}
</div>
{#each uniqBy(rows, (r) => r.id) as r (r.id)}
  {@const b = r.body}
  {@const deal = b.battle.verdict === 'deal'}
  <article class="card">
    <header>
      <strong>{dirText(b.direction)}</strong>
      <span class="tag">{deal ? `сделка ${b.battle.deal_id}` : verdictLabel(b.battle.verdict)}</span>
      {#if !deal && b.battle.reason}<span class="muted">{b.battle.reason}</span>{/if}
      <span class="muted">{showTime(deal ? b.at : b.ep.first_at)}</span>
    </header>
    <p>
      {#if deal}
        Зашли на <strong>{money(b.battle.invested)} USDT</strong> (бюджет {money(b.budget)}), план {money(b.battle.profit)} USDT{#if r.fact}, факт
          {#if r.fact.result_usd != null}<strong class={Number(r.fact.result_usd) < 0 ? 'err' : 'ok'}>{money(r.fact.result_usd)} USDT</strong>{:else}<span class="muted">ещё нет</span>{/if}{/if};
      {:else}
        На нашем бюджете {money(b.budget)} USDT: {#if b.at_budget?.at}вложить {money(b.at_budget.at.invested)} → прибыль {money(b.at_budget.at.profit)} USDT{#if !b.at_budget.passes} <span class="warn-text">— ниже порогов, сделки нет</span>{/if}{:else}<span class="muted">объёма нет</span>{/if};
      {/if}
      без потолка: вложить <strong>{money(b.opt.invested)}</strong> → прибыль <strong>{money(b.opt.profit)}</strong> USDT{#if !b.opt_passes} <span class="warn-text">(ниже порогов)</span>{/if};
      упущено <strong class={Number(b.missed) > 0 ? 'err' : ''}>{money(b.missed)} USDT</strong>;
      ограничило: <span class={budgetBound(b.limited_by) ? 'warn-text' : ''}>{limitedLabel(b.limited_by)}</span>
      <button type="button" onclick={() => (open = open === r.id ? null : r.id)}>{open === r.id ? 'Скрыть' : 'Подробнее'}</button>
    </p>
    <p class="muted">
      {#if deal}
        Окно: {#if b.battle.window_from}спред выше порога с {showTime(b.battle.window_from)}, до входа {duration(windowSec(b))}{:else}начало окна не записано (спред не был выше порога или запись до решения 197){/if}
      {:else}
        Окно: {showTime(b.ep.first_at)} — {showTime(b.ep.last_at)} ({duration(windowSec(b))}), расчётов с упущенным {b.ep.count}, пик {showTime(b.ep.peak_at)}{#if b.battle.window_from}; спред выше порога с {showTime(b.battle.window_from)}{/if}
      {/if}
    </p>
    {#if open === r.id}<UncappedRecord rec={b} />{/if}
  </article>
{:else}
  {#if limit}<p class="muted">За период записей нет.</p>{:else}<p class="muted">Задайте «Строк на странице».</p>{/if}
{/each}
