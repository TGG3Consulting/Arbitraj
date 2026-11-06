<!--
  Файл: ui/src/pages/Deals.svelte
  Что делает: экран «Сделки» — идущие и закрытые сделки: направление, шаг (покупка, перевод, продажа,
    застрявшая монета, ждёт человека…), где сейчас монеты и сколько, вложено, выручка, затраты, итог против
    плана; раскрывается хронология шагов и план (полный расчёт затрат). Данные — GET /api/deals,
    живое сообщение deal обновляет строку на месте. Решение 123 («лучший из двух»): под строкой — кто дал финальную
    котировку DEX в плане (Quote.pick); кто исполнил своп и почему — в хронологии («исполнил: свой сборщик: … /
    KyberSwap; лучший из двух: …»). Решение 141: «зашли на X → потенциал без потолка Y и прибыль Z, упущено» (DealUncapped).
    Решения 138, 163: строка возврата выручки USDT — идёт / ждёт повтора / вернулись / перегнаны вручную / не вернулись.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/CostLines.svelte, ErrorText.svelte,
    components/DealUncapped.svelte.
  Настройки: нет.
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import { stepLabel, dirText, pickLines } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import ErrorText from '../components/ErrorText.svelte';
  import CostLines from '../components/CostLines.svelte';
  import DealUncapped from '../components/DealUncapped.svelte';

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
      else if (lagFor(msg, 'core')) reload();
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
<!-- Решение 193: отчёт «Итоги сделок» (xlsx) 1 в 1 как отчёт 05.11.2026, на сделках на момент скачивания. -->
<p><a href="/api/report/results.xlsx" download>Скачать отчёт «Итоги сделок» (xlsx)</a></p>
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
    <!-- Решение 138: возврат выручки USDT на площадку покупки после закрытия (поле сделки usdt_return). -->
    {#if d.usdt_return?.state === 'pending'}
      <p class="muted">Возврат USDT: идёт — {d.usdt_return.amount} USDT <code>{d.usdt_return.from}</code> → <code>{d.usdt_return.to}</code></p>
    {:else if d.usdt_return?.state === 'done'}
      <p class="ok">USDT вернулись: {d.usdt_return.amount} USDT <code>{d.usdt_return.from}</code> → <code>{d.usdt_return.to}</code>, зачислено {d.usdt_return.credited}</p>
    {:else if d.usdt_return?.state === 'retrying'}
      <!-- Решение 163: ждёт повтора (usdt.return_retry_s, usdt.return_retries_max). -->
      <p class="muted">Возврат USDT: ждёт повтора — {d.usdt_return.amount} USDT <code>{d.usdt_return.from}</code> → <code>{d.usdt_return.to}</code>, повторов сделано {d.usdt_return.tries}; последняя причина: {d.usdt_return.why}</p>
    {:else if d.usdt_return?.state === 'manual'}
      <p class="ok">USDT перегнаны вручную: {d.usdt_return.amount} USDT <code>{d.usdt_return.at}</code> → <code>{d.usdt_return.to}</code> ({d.usdt_return.by})</p>
    {:else if d.usdt_return?.state === 'not_returned'}
      <p class="err">USDT не вернулись: {d.usdt_return.amount} USDT лежат на <code>{d.usdt_return.at}</code> — {d.usdt_return.why}. Перегоните на площадку покупки — кнопки «Перегнать» и «Перегнано вручную» на экране «Ручное управление»</p>
    {/if}
    {#if d.buy}<DealUncapped id={d.id} fact={d.result_usd} />{/if}
    <details>
      <summary>Хронология ({d.timeline.length})</summary>
      <table><tbody>
        {#each d.timeline as [at, text], i (i)}<tr><td>{showTime(at)}</td><td>{text}</td></tr>{/each}
      </tbody></table>
    </details>
    {#each pickLines(d.plan) as line, i (i)}
      <p class="muted">План, исполнитель DEX (решение 123), {line}</p>
    {/each}
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
