<!--
  Файл: ui/src/pages/Deals.svelte
  Что делает: экран «Сделки» — идущие и закрытые сделки: направление, шаг (покупка, перевод, продажа,
    застрявшая монета, ждёт человека…), где сейчас монеты и сколько, вложено, выручка, затраты, итог против
    плана; раскрывается хронология шагов и план (полный расчёт затрат). Данные — GET /api/deals,
    живое сообщение deal обновляет строку на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/CostLines.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { stepLabel, dirText } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import ErrorText from '../components/ErrorText.svelte';
  import CostLines from '../components/CostLines.svelte';

  let deals = $state({});
  let error = $state('');
  let onlyOpen = $state(false);
  const OPEN = ['Buying', 'Transfer', 'Selling', 'Stuck', 'WaitingHuman'];

  /** Перечитывает сделки. Ошибка — в error. */
  async function reload() {
    try {
      deals = Object.fromEntries((await get('/api/deals')).map((d) => [d.id, d]));
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    reload();
    return onLive((msg) => {
      if (msg.type === 'deal') deals[msg.data.id] = msg.data;
      else if (msg.type === 'lagged') reload();
    });
  });

  const rows = $derived(
    Object.values(deals)
      .filter((d) => !onlyOpen || OPEN.includes(d.step))
      .sort((a, b) => (b.started_at > a.started_at ? 1 : -1)),
  );
</script>

<h1>Сделки</h1>
<ErrorText text={error} />
<label class="inline"><input type="checkbox" bind:checked={onlyOpen} /> только идущие</label>
{#each rows as d (d.id)}
  <article class="card">
    <header>
      <strong>{dirText(d.direction)}</strong>
      <span class="tag">{stepLabel(d.step)}</span>
      <code>{d.id}</code>
      <span class="muted">с {showTime(d.started_at)}</span>
    </header>
    <p>
      Монеты: {d.qty} на <code>{d.at}</code> · вложено {d.invested} · выручка {d.proceeds} · затраты {d.spent} USDT ·
      план {d.plan_profit_usd} USDT ·
      {#if d.result_usd != null}<span class={String(d.result_usd).startsWith('-') ? 'err' : 'ok'}>итог {d.result_usd} USDT</span>
      {:else}<span class="muted">итога ещё нет</span>{/if}
      {#if d.relays}<span class="muted"> · перегонов {d.relays}</span>{/if}
      {#if d.dust && Number(d.dust) > 0}<span class="muted"> · пыль {d.dust} на <code>{d.at}</code> (меньше шага или минимума ордера, решение 66)</span>{/if}
    </p>
    <details>
      <summary>Хронология ({d.timeline.length})</summary>
      <table><tbody>
        {#each d.timeline as [at, text], i (i)}<tr><td>{showTime(at)}</td><td>{text}</td></tr>{/each}
      </tbody></table>
    </details>
    {#if d.plan?.costs}
      <details>
        <summary>План: объём {d.plan.qty}, прибыль {d.plan.profit_usd} USDT</summary>
        <CostLines lines={d.plan.costs} />
      </details>
    {/if}
  </article>
{:else}
  <p class="muted">Сделок ещё не было. Новые сделки открываются только в режиме «В работе» (кнопка «Старт» вверху).</p>
{/each}
