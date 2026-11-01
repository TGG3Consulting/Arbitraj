<!--
  Файл: ui/src/pages/Manual.svelte
  Что делает: экран «Ручное управление» (решение 37): алерты застрявших монет (сделка, площадка, монета,
    количество, убыток %, время) с кнопками «Продать сейчас» и «Беру на себя»; монеты «Беру на себя»
    (сделка, площадка, монета, количество, причина, когда взяты) с кнопкой «Вернуть боту» (возврат по реальному
    балансу площадки — считает ядро). Данные — GET /api/manual, команды — POST /api/manual/command
    (источник — интерфейс, пишется в журнал). Пока ядро не подключено — «нет данных» (503) с причиной.
    Решение 100: остановленные сделки (продажа не прошла, исход неизвестен, прервана перезапуском) — «Продать сейчас»
    (бот продаёт по рынку то, что реально на площадке) и «Продано вручную» (человек продал сам и вводит выручку
    USDT — сделка закрывается фактом вместо предварительного убытка). «Продано вручную» есть и у алертов, и у «Беру на себя».
    Решение 101: «Продать и перегнать» (у алерта и остановленной сделки) — продажа по рынку, затем выручка USDT
    переводится на площадку покупки (откуда пришли деньги), независимо от режима возврата USDT.
    Живые сообщения stuck_alert, manual, deal — перечитать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/remote.svelte.js, lib/coinLabels.js, lib/labels.js, components/NoData.svelte, ErrorText.svelte.
  Настройки: нет (режим и пороги — stuck.*, экран «Настройки»).
-->
<script>
  import { post } from '../lib/api.js';
  import { remote } from '../lib/remote.svelte.js';
  import { assetLabel } from '../lib/coinLabels.js';
  import { showTime } from '../lib/labels.js';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  const manual = remote('/api/manual', ['stuck_alert', 'manual', 'deal']);
  let busy = $state(false);
  let result = $state(null);

  /**
   * Отправляет команду ядру. Принимает: ManualAction ({"SellNow":{"alert_id":…}} и т.п.) и вопрос подтверждения.
   * Результат или причина отказа — над таблицами; после ответа данные перечитываются.
   */
  async function command(action, question) {
    if (!confirm(question)) return;
    busy = true;
    try {
      result = await post('/api/manual/command', { action });
    } catch (e) {
      result = { ok: false, details: e.message };
    }
    busy = false;
    manual.reload();
  }

  /**
   * «Продано вручную»: спрашивает выручку USDT и отправляет SoldManually. Принимает: id сделки, монету, количество.
   * Пустой ввод или «Отмена» — ничего не отправляется; число проверяет ядро.
   */
  async function soldManually(dealId, ticker, qty) {
    const usdt = prompt(`Сколько USDT вы получили за ${qty} ${ticker}? Сделка ${dealId} закроется этой выручкой.`);
    if (usdt === null || usdt.trim() === '') return;
    await command({ SoldManually: { deal_id: dealId, usdt: usdt.trim() } }, `Закрыть сделку ${dealId}: выручка ${usdt.trim()} USDT?`);
  }

  /** Последняя запись хронологии сделки — причина остановки. */
  function lastNote(d) {
    const t = d.timeline || [];
    const notes = t.filter(([, x]) => !x.startsWith('Предварительный убыток'));
    return notes.length ? notes[notes.length - 1][1] : '';
  }
</script>

<h1>Ручное управление</h1>
<NoData text={manual.noData} />
<ErrorText text={manual.error} />
{#if result}<p class={result.ok ? 'ok' : 'err'} role="status">{result.details}</p>{/if}

{#if manual.loading}
  <p class="muted">Загружаю…</p>
{:else if manual.data}
  <h2>Застрявшие монеты — ждут решения</h2>
  <table>
    <thead><tr><th>Монета</th><th>Площадка</th><th class="num">Количество</th><th class="num">Убыток, %</th><th>Причина</th><th>Сделка</th><th>Когда</th><th></th></tr></thead>
    <tbody>
      {#each manual.data.alerts as a (a.id)}
        <tr>
          <td>{assetLabel(a.asset)}</td><td>{a.venue}</td><td class="num">{a.qty}</td><td class="num err">{a.loss_pct ?? '—'}</td>
          <td>{a.reason ?? ''}</td><td><code>{a.deal_id}</code></td><td>{showTime(a.at)}</td>
          <td class="row">
            <button disabled={busy} onclick={() => command({ SellNow: { alert_id: a.id } }, `Продать ${a.qty} ${a.asset.ticker} на ${a.venue} по рынку сейчас?`)}>Продать сейчас</button>
            <button disabled={busy} onclick={() => command({ SellNowReturn: { alert_id: a.id } }, `Продать ${a.qty} ${a.asset.ticker} на ${a.venue} по рынку и перегнать выручку USDT на площадку покупки?`)}>Продать и перегнать</button>
            <button class="quiet" disabled={busy} onclick={() => command({ TakeOver: { alert_id: a.id } }, `Забрать ${a.qty} ${a.asset.ticker} на себя? Бот спишет их из учёта.`)}>Беру на себя</button>
            <button class="quiet" disabled={busy} onclick={() => soldManually(a.deal_id, a.asset.ticker, a.qty)}>Продано вручную</button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="8" class="muted">Алертов нет.</td></tr>
      {/each}
    </tbody>
  </table>

  <h2>Монеты «Беру на себя»</h2>
  <table>
    <thead><tr><th>Монета</th><th>Площадка</th><th class="num">Количество</th><th>Причина</th><th>Сделка</th><th>Взяты</th><th></th></tr></thead>
    <tbody>
      {#each manual.data.holdings as h (h.id)}
        <tr>
          <td>{assetLabel(h.asset)}</td><td>{h.venue}</td><td class="num">{h.qty}</td><td>{h.reason}</td>
          <td><code>{h.deal_id}</code></td><td>{showTime(h.taken_at)}</td>
          <td class="row">
            <button disabled={busy} onclick={() => command({ ReturnToBot: { holding_id: h.id } }, `Вернуть ${h.asset.ticker} боту? Учёт — по реальному балансу ${h.venue}.`)}>Вернуть боту</button>
            <button class="quiet" disabled={busy} onclick={() => soldManually(h.deal_id, h.asset.ticker, h.qty)}>Продано вручную</button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="7" class="muted">Монет на ручном управлении нет.</td></tr>
      {/each}
    </tbody>
  </table>

  <h2>Остановленные сделки — ждут разбора</h2>
  <p class="muted">Пока сделка не разобрана, в итоге дня стоит предварительный убыток (монеты считаются потерянными) — он может держать систему в «Стоп» по дневному лимиту.</p>
  <table>
    <thead><tr><th>Монета</th><th>Площадка</th><th class="num">Количество</th><th class="num">Вложено</th><th class="num">Предв. убыток</th><th>Причина</th><th>Сделка</th><th></th></tr></thead>
    <tbody>
      {#each manual.data.halted ?? [] as d (d.id)}
        <tr>
          <td>{assetLabel(d.direction.asset)}</td><td>{d.at}</td><td class="num">{d.qty}</td><td class="num">{d.invested}</td>
          <td class="num err">{d.provisional_usd ?? '—'}</td><td>{lastNote(d)}</td><td><code>{d.id}</code></td>
          <td class="row">
            <button disabled={busy || !(Number(d.qty) > 0)} onclick={() => command({ SellHalted: { deal_id: d.id } }, `Продать ${d.direction.asset.ticker} сделки ${d.id} на ${d.at} по рынку сейчас? Количество — по реальному балансу.`)}>Продать сейчас</button>
            <button disabled={busy || !(Number(d.qty) > 0) || d.at === d.direction.buy_on} onclick={() => command({ SellHaltedReturn: { deal_id: d.id } }, `Продать ${d.direction.asset.ticker} сделки ${d.id} на ${d.at} и перегнать выручку USDT на ${d.direction.buy_on}?`)}>Продать и перегнать</button>
            <button class="quiet" disabled={busy} onclick={() => soldManually(d.id, d.direction.asset.ticker, d.qty)}>Продано вручную</button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="8" class="muted">Остановленных сделок нет.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}
