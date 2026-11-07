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
    Решение 156: кнопки строк рисуются из records (GET /api/manual: у каждой записи — кнопки, команда ядра, вопрос
    подтверждения, ввод суммы, недоступность с причиной; core/manual_feed.rs) — ровно те же уходят в Telegram.
    Решение 163: «USDT не вернулись — ждут перегона» (usdt из GET /api/manual): закрытая сделка, выручка USDT не
    вернулась на площадку покупки (повторы usdt.return_retry_s / usdt.return_retries_max кончились, исход неизвестен
    или повторов нет) — сделка, сумма, где лежат, куда должны, причина; кнопки «Перегнать» (ещё одна попытка
    возврата) и «Перегнано вручную» (человек перевёл сам). Кнопок «Продать» нет — уже продано.
    Решение 164: числа округлены (USDT — 2 знака, количество — 4 значащие цифры, lib/fmt.js), «Причина» — короткий
    человеческий текст из records (reason), полный исходный текст — по наведению (reason_full).
    Решение 121 (C4): внизу — «Сухая сделка DEX» (components/DexDrySwap.svelte): проверка своего свопа без отправки,
    не зависит от данных ядра.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/remote.svelte.js, lib/fmt.js, lib/coinLabels.js, lib/labels.js, components/NoData.svelte, ErrorText.svelte, DexDrySwap.svelte.
  Настройки: нет (режим и пороги — stuck.*, экран «Настройки»).
-->
<script>
  import { uniqBy } from '../lib/uniq.js';
  import { post } from '../lib/api.js';
  import { remote } from '../lib/remote.svelte.js';
  import { assetLabel } from '../lib/coinLabels.js';
  import { showTime } from '../lib/labels.js';
  import { fmtUsd, fmtQty } from '../lib/fmt.js';
  import NoData from '../components/NoData.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import DexDrySwap from '../components/DexDrySwap.svelte';

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

  /** Кнопки записи экрана по ключу («a.<id>», «h.<id>», «d.<id>», «u.<id>») — из records (решения 156, 163). */
  function buttonsOf(key) {
    return (manual.data?.records ?? []).find((r) => r.key === key)?.buttons ?? [];
  }

  /**
   * Нажатие кнопки записи: с вводом («Продано вручную») — спрашивает выручку USDT (пустой ввод или «Отмена» —
   * ничего не отправляется; число проверяет ядро) и подставляет её в команду и вопрос; затем confirm и команда ядру.
   */
  async function press(b) {
    let action = b.action;
    let question = b.confirm;
    if (b.input) {
      const usdt = prompt(b.input);
      if (usdt === null || usdt.trim() === '') return;
      const key = Object.keys(action)[0];
      action = { [key]: { ...action[key], usdt: usdt.trim() } };
      question = question.replace('{usdt}', usdt.trim());
    }
    await command(action, question);
  }

  /** Запись экрана по ключу — причина коротко (reason) и полностью (reason_full), решение 164. */
  function recordOf(key) {
    return (manual.data?.records ?? []).find((r) => r.key === key) ?? {};
  }
</script>

{#snippet buttons(key)}
  {#each buttonsOf(key) as b, bi (`${b.code}|${bi}`)}
    <button class={b.quiet ? 'quiet' : ''} disabled={busy || !!b.disabled} title={b.disabled ?? ''} onclick={() => press(b)}>{b.title}</button>
  {/each}
{/snippet}

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
      {#each uniqBy(manual.data.alerts, (a) => a.id) as a (a.id)}
        <tr>
          <td>{assetLabel(a.asset)}</td><td>{a.venue}</td><td class="num" title={a.qty}>{fmtQty(a.qty)}</td><td class="num err" title={a.loss_pct ?? ''}>{a.loss_pct == null ? '—' : Number(a.loss_pct).toFixed(2)}</td>
          <td title={recordOf('a.' + a.id).reason_full ?? a.reason ?? ''}>{recordOf('a.' + a.id).reason ?? a.reason ?? ''}</td><td><code>{a.deal_id}</code></td><td>{showTime(a.at)}</td>
          <td class="row">{@render buttons('a.' + a.id)}</td>
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
      {#each uniqBy(manual.data.holdings, (h) => h.id) as h (h.id)}
        <tr>
          <td>{assetLabel(h.asset)}</td><td>{h.venue}</td><td class="num" title={h.qty}>{fmtQty(h.qty)}</td><td title={recordOf('h.' + h.id).reason_full ?? h.reason}>{recordOf('h.' + h.id).reason ?? h.reason}</td>
          <td><code>{h.deal_id}</code></td><td>{showTime(h.taken_at)}</td>
          <td class="row">{@render buttons('h.' + h.id)}</td>
        </tr>
      {:else}
        <tr><td colspan="7" class="muted">Монет на ручном управлении нет.</td></tr>
      {/each}
    </tbody>
  </table>

  <h2>Остановленные сделки — ждут разбора</h2>
  <p class="muted">Пока сделка не разобрана, в итоге дня стоит оценка позиции: стоимость монет по рынку площадки, где они лежат, минус вложено (решение 152); цены нет — худший случай (вся сумма). Оценка может держать систему в «Стоп» по дневному лимиту; после разбора заменяется фактом.</p>
  <table>
    <thead><tr><th>Монета</th><th>Площадка</th><th class="num">Количество</th><th class="num">Вложено</th><th class="num">Оценка в итоге дня</th><th>Причина</th><th>Сделка</th><th></th></tr></thead>
    <tbody>
      {#each uniqBy(manual.data.halted, (d) => d.id) as d (d.id)}
        <tr>
          <td>{assetLabel(d.direction.asset)}</td><td>{d.at}</td><td class="num" title={d.qty}>{fmtQty(d.qty)}</td><td class="num" title={d.invested}>{fmtUsd(d.invested)}</td>
          <td class="num err" title={d.provisional_usd ?? ''}>{fmtUsd(d.provisional_usd)}</td><td title={recordOf('d.' + d.id).reason_full ?? ''}>{recordOf('d.' + d.id).reason ?? ''}</td><td><code>{d.id}</code></td>
          <td class="row">{@render buttons('d.' + d.id)}</td>
        </tr>
      {:else}
        <tr><td colspan="8" class="muted">Остановленных сделок нет.</td></tr>
      {/each}
    </tbody>
  </table>

  <h2>USDT не вернулись — ждут перегона</h2>
  <p class="muted">Сделка закрыта, а выручка USDT не вернулась на площадку покупки и бот сам больше не вернёт (повторы кончились, исход неизвестен или повторов нет). «Перегнать» — ещё одна попытка: свободные USDT площадки, не больше суммы. «Перегнано вручную» — вы перевели сами, запись закроется.</p>
  <table>
    <thead><tr><th>Сделка</th><th class="num">Сумма, USDT</th><th>Где лежат</th><th>Куда должны</th><th>Причина</th><th></th></tr></thead>
    <tbody>
      {#each uniqBy(manual.data.usdt, (d) => d.id) as d (d.id)}
        <tr>
          <td><code>{d.id}</code></td><td class="num">{d.usdt_return.amount}</td><td>{d.usdt_return.at}</td>
          <td>{d.direction.buy_on}</td><td class="err">{d.usdt_return.why}</td>
          <td class="row">{@render buttons('u.' + d.id)}</td>
        </tr>
      {:else}
        <tr><td colspan="6" class="muted">Невернувшихся USDT нет.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}

<DexDrySwap />
