<!--
  Файл: ui/src/components/PoolCondStat.svelte
  Что делает: решение 439 — блок «Условие пулов: статистика» площадки DEX на экране «Каналы отправки»: режим
    send.pool_cond_mode сейчас (off — без условия пулов, только окно времени; pools — с условием пулов маршрута; не
    задана — как off) и причина, если значение не разобрано; таблица по режимам на момент отправки сделок цикла через
    исполнитель: отправки, победы, откаты и оплаченный ими газ (нативная монета), бесплатные отказы по условию пулов,
    отказы по окну времени, прочие итоги; кнопка «Сбросить статистику». По таблице Тигран решает, включать ли pools.
    Перечитывается при открытии, по «Обновить», после сброса и по записям журнала send.pool_cond_stat (живой поток) —
    без таймеров. Площадка не eip1559_l1 (BSC) — API отвечает 400 (NotApplicable), блок не показывается.
  Кто вызывает: pages/Channels.svelte.
  От чего зависит: lib/api.js (get, request, seg), lib/live.svelte.js (onLive), components/ErrorText.svelte.
  Настройки: send.pool_cond_mode (задаётся в «Настройках площадки», группа «Отправка»).
-->
<script>
  import { get, request, seg } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import ErrorText from './ErrorText.svelte';

  /** venue — id площадки DEX. */
  let { venue } = $props();
  let st = $state(null);
  let error = $state('');
  let busy = $state(false);
  /** Площадка не eip1559_l1 (400 NotApplicable) — блока нет. */
  let na = $state(false);

  /** Что делает: читает GET /api/venues/{id}/pool-cond-stat. Ошибки — в error («не применимо» — пусто). */
  async function load() {
    try {
      st = await get(`/api/venues/${seg(venue)}/pool-cond-stat`);
      error = '';
      na = false;
    } catch (e) {
      if (e.aborted) return;
      st = null;
      na = e.status === 400;
      error = na ? '' : e.message;
    }
  }

  /** Что делает: «Сбросить статистику» — POST …/pool-cond-stat/reset. Ошибки — в error. */
  async function reset() {
    busy = true;
    try {
      st = await request('POST', `/api/venues/${seg(venue)}/pool-cond-stat/reset`);
      error = '';
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    load();
    return onLive((msg) => {
      const k = msg.type === 'journal' ? msg.data?.kind : '';
      if (k === 'send.pool_cond_stat') load();
    });
  });

  /** Прочие итоги режима: отправки минус учтённые виды. */
  const other = (c) => c.sends - c.wins - c.reverts - c.rejected_pools - c.rejected_time;
  const modes = [
    ['off', 'off — без условия пулов (только окно времени)'],
    ['pools', 'pools — с условием пулов маршрута'],
  ];
</script>

{#if !na}
<h3>Условие пулов: статистика — решение 439</h3>
{/if}
{#if st}
  <p class={st.note ? 'err' : 'muted'}>
    Режим send.pool_cond_mode: {st.mode ?? 'не задана — условия пулов нет (как off)'}{st.note ? ` — не разобран: ${st.note}; условия пулов нет` : ''}.
    Сделки считаются по режиму в действии на момент отправки (pools — только при канале sequencer с окном channel.cond_time_window_s).
    {st.since ? `Отсчёт с ${st.since}.` : ''}
  </p>
  {#if st.channels_note}<p class="err">{st.channels_note}</p>{/if}
  <table>
    <thead>
      <tr>
        <th>Режим</th><th>Отправки</th><th>Победы</th><th>Откаты</th><th>Газ откатов (нативная монета)</th>
        <th>Отказы по условию пулов (бесплатно)</th><th>Отказы по окну времени</th><th>Прочие итоги</th>
      </tr>
    </thead>
    <tbody>
      {#each modes as [key, title] (key)}
        {@const c = st[key]}
        <tr>
          <td>{title}</td><td>{c.sends}</td><td>{c.wins}</td><td>{c.reverts}</td>
          <td>{c.revert_gas}{c.revert_gas_unknown ? ` (+ ${c.revert_gas_unknown} без квитанции)` : ''}</td>
          <td>{c.rejected_pools}</td><td>{c.rejected_time}</td><td>{other(c)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  <p class="muted">Каждый итог — строка журнала send.pool_cond_stat. Прочие итоги — сбои до отправки секвенсору (ошибка канала, предел газа Solidly — транзакция до секвенсора не дошла), «не включена», «исход неизвестен».</p>
  <p>
    <button onclick={load}>Обновить</button>
    <button onclick={reset} disabled={busy}>Сбросить статистику</button>
  </p>
{/if}
<ErrorText text={error} />
