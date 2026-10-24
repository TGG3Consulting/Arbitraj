<!--
  Файл: ui/src/pages/Live.svelte
  Что делает: «Живой экран» — одна строка на монету (решение 55): спред в % в обе стороны рядом
    («купить на A → продать на B» и обратно) по каждой паре площадок из данных — спред, порог комиссий,
    состояние фильтра шума с причиной, цены; сортировка по наибольшему спреду; и последние расчёты кандидатов:
    прошёл (прибыль, объём) или отказ с причиной; по кандидату раскрывается полный список затрат.
    Данные — GET /api/spreads, /api/candidates; живые сообщения spread и candidate обновляют строки на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/ErrorText.svelte,
    components/LiveCoins.svelte, components/CostLines.svelte.
  Настройки: нет (пороги signal.*, money.*, profit.* — экран «Настройки»).
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { dirText, dirKey } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import ErrorText from '../components/ErrorText.svelte';
  import CostLines from '../components/CostLines.svelte';
  import LiveCoins from '../components/LiveCoins.svelte';

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

  /** Фильтр по тикеру монеты (Asset). */
  const matchAsset = (a) => a.ticker.toLowerCase().includes(filter.trim().toLowerCase());
  const match = (d) => matchAsset(d.asset);
  const views = $derived(Object.values(spreads));
  const candRows = $derived(Object.values(cands).filter((c) => match(c.direction)).sort((a, b) => (b.at > a.at ? 1 : -1)));
</script>

<h1>Живой экран</h1>
<ErrorText text={error} />
<div class="row">
  <input placeholder="монета" bind:value={filter} />
  <label class="inline"><input type="checkbox" bind:checked={onlyActive} /> только выше порога</label>
</div>

<h2>Спреды по монетам: обе стороны</h2>
<LiveCoins {views} match={matchAsset} {onlyActive} />

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
