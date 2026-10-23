<!--
  Файл: ui/src/pages/Manual.svelte
  Что делает: экран «Ручное управление» (решение 37): алерты застрявших монет (сделка, площадка, монета,
    количество, убыток %, время) с кнопками «Продать сейчас» и «Беру на себя»; монеты «Беру на себя»
    (сделка, площадка, монета, количество, причина, когда взяты) с кнопкой «Вернуть боту» (возврат по реальному
    балансу площадки — считает ядро). Данные — GET /api/manual, команды — POST /api/manual/command
    (источник — интерфейс, пишется в журнал). Пока ядро не подключено — «нет данных» (503) с причиной.
    Живые сообщения stuck_alert, manual — перечитать.
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

  const manual = remote('/api/manual', ['stuck_alert', 'manual']);
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
    <thead><tr><th>Монета</th><th>Площадка</th><th class="num">Количество</th><th class="num">Убыток, %</th><th>Сделка</th><th>Когда</th><th></th></tr></thead>
    <tbody>
      {#each manual.data.alerts as a (a.id)}
        <tr>
          <td>{assetLabel(a.asset)}</td><td>{a.venue}</td><td class="num">{a.qty}</td><td class="num err">{a.loss_pct}</td>
          <td><code>{a.deal_id}</code></td><td>{showTime(a.at)}</td>
          <td class="row">
            <button disabled={busy} onclick={() => command({ SellNow: { alert_id: a.id } }, `Продать ${a.qty} ${a.asset.ticker} на ${a.venue} по рынку сейчас?`)}>Продать сейчас</button>
            <button class="quiet" disabled={busy} onclick={() => command({ TakeOver: { alert_id: a.id } }, `Забрать ${a.qty} ${a.asset.ticker} на себя? Бот спишет их из учёта.`)}>Беру на себя</button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="7" class="muted">Алертов нет.</td></tr>
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
          <td><button disabled={busy} onclick={() => command({ ReturnToBot: { holding_id: h.id } }, `Вернуть ${h.asset.ticker} боту? Учёт — по реальному балансу ${h.venue}.`)}>Вернуть боту</button></td>
        </tr>
      {:else}
        <tr><td colspan="7" class="muted">Монет на ручном управлении нет.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}
