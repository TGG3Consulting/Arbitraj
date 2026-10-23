<!--
  Файл: ui/src/pages/Live.svelte
  Что делает: «Живой экран» — спреды по всем направлениям на каждый тик (покупка на A, продажа на B,
    грубый спред, порог комиссий, состояние фильтра шума с причиной) и последние расчёты кандидатов:
    прошёл (прибыль, объём) или отказ с причиной; по кандидату раскрывается полный список затрат.
    Данные — GET /api/spreads, /api/candidates; живые сообщения spread и candidate обновляют строки на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/ErrorText.svelte.
  Настройки: нет (пороги signal.*, money.*, profit.* — экран «Настройки»).
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { spreadState, dirText, dirKey } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import ErrorText from '../components/ErrorText.svelte';
  import CostLines from '../components/CostLines.svelte';

  let spreads = $state({});
  let cands = $state({});
  let error = $state('');
  let filter = $state('');
  let onlyActive = $state(false);

  /** Перечитывает оба списка. Ошибка — в error. */
  async function reload() {
    try {
      const [s, c] = await Promise.all([get('/api/spreads'), get('/api/candidates')]);
      spreads = Object.fromEntries(s.map((v) => [dirKey(v.direction), v]));
      cands = Object.fromEntries(c.map((v) => [dirKey(v.direction), v]));
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    reload();
    return onLive((msg) => {
      if (msg.type === 'spread') spreads[dirKey(msg.data.direction)] = msg.data;
      else if (msg.type === 'candidate') cands[dirKey(msg.data.direction)] = msg.data;
      else if (msg.type === 'lagged' || msg.type === 'coins') reload();
    });
  });

  const match = (d) => d.asset.ticker.toLowerCase().includes(filter.trim().toLowerCase());
  const rows = $derived(
    Object.values(spreads)
      .filter((v) => match(v.direction) && (!onlyActive || ['Holding', 'Candidate', 'Anomaly'].includes(v.state)))
      .sort((a, b) => Number(b.spread_pct ?? -1e9) - Number(a.spread_pct ?? -1e9)),
  );
  const candRows = $derived(Object.values(cands).filter((c) => match(c.direction)).sort((a, b) => (b.at > a.at ? 1 : -1)));
</script>

<h1>Живой экран</h1>
<ErrorText text={error} />
<div class="row">
  <input placeholder="монета" bind:value={filter} />
  <label class="inline"><input type="checkbox" bind:checked={onlyActive} /> только выше порога</label>
</div>

<h2>Спреды по направлениям</h2>
<table>
  <thead><tr><th>Направление</th><th class="num">Покупка на A</th><th class="num">Продажа на B</th><th class="num">Спред, %</th><th class="num">Порог комиссий, %</th><th>Состояние</th><th>Обновлено</th></tr></thead>
  <tbody>
    {#each rows as v (dirKey(v.direction))}
      {@const st = spreadState(v.state)}
      <tr>
        <td>{dirText(v.direction)}</td>
        <td class="num">{v.prices?.buy ?? '—'}</td>
        <td class="num">{v.prices?.sell ?? '—'}</td>
        <td class="num" class:ok={Number(v.spread_pct) > Number(v.prices?.fees_pct)}>{v.spread_pct ?? '—'}</td>
        <td class="num">{v.prices?.fees_pct ?? '—'}</td>
        <td class={st.cls}>{st.text}{v.updates ? ` · ${v.updates} обн.` : ''}</td>
        <td>{showTime(v.at)}</td>
      </tr>
    {:else}
      <tr><td colspan="7" class="muted">Направлений нет: справочник пуст или ни одна пара площадок ещё не прошла первичный фильтр (экран «Монеты»).</td></tr>
    {/each}
  </tbody>
</table>

<h2>Кандидаты: расчёт и причины отказа</h2>
{#each candRows as c (dirKey(c.direction))}
  {@const acc = c.verdict.Accept}
  {@const rej = c.verdict.Reject}
  {@const calc = acc ?? rej?.best}
  <article class="card">
    <header>
      <strong>{dirText(c.direction)}</strong>
      {#if acc}<span class="ok">прошёл: прибыль {acc.profit_usd} USDT ({acc.profit_pct}%), объём {acc.qty}</span>
      {:else}<span class="err">отказ: {rej.reason}</span>{/if}
      <span class="muted">{showTime(c.at)}</span>
    </header>
    {#if calc}
      <details>
        <summary>Полный расчёт: вложено {calc.invested}, выручка {calc.proceeds}, затраты {calc.costs_usdt} USDT, T = {calc.transit_s} с, σ = {calc.sigma}</summary>
        <CostLines lines={calc.costs} />
      </details>
    {/if}
  </article>
{:else}
  <p class="muted">Кандидатов ещё не было.</p>
{/each}
