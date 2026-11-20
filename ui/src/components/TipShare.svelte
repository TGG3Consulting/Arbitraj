<!--
  Файл: ui/src/components/TipShare.svelte
  Что делает: решение 437 (3) — блок «Доля чаевых (PGA)» площадки DEX на экране «Каналы отправки»: режим send.tip_mode
    (fixed / profit_share) и причина, если режим не работает (не заданы настройки); текущая доля прибыли на чаевые,
    чаевые последней сделки, счётчики (победы, проигрыши по чаевым, по скорости, побед подряд), последний проигрыш
    (наша транзакция, блок, перехватчик и его чаевые, наши чаевые); кнопка «Сбросить долю к минимуму». Каждый проигрыш
    — и в журнале (send.tip_loss). Перечитывается при открытии, по «Обновить», после сброса и по записям журнала
    send.tip_loss / send.tip_share (живой поток) — без таймеров.
  Кто вызывает: pages/Channels.svelte.
  От чего зависит: lib/api.js (get, request, seg), lib/live.svelte.js (onLive), components/ErrorText.svelte.
  Настройки: send.tip_mode, send.tip_floor_gwei, send.tip_cap_gwei, send.tip_share_min_pct, send.tip_share_max_pct,
    send.tip_share_step_up_pct, send.tip_share_step_down_pct, send.tip_wins_to_step_down (задаются в «Настройках
    площадки», группа «Отправка»).
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

  /** Что делает: читает GET /api/venues/{id}/tip-share. Ошибки — в error («не применимо» — пусто). */
  async function load() {
    try {
      st = await get(`/api/venues/${seg(venue)}/tip-share`);
      error = '';
    } catch (e) {
      if (e.aborted) return;
      st = null;
      error = e.status === 400 ? '' : e.message;
    }
  }

  /** Что делает: «Сбросить долю к минимуму» — POST …/tip-share/reset. Ошибки — в error. */
  async function reset() {
    busy = true;
    try {
      st = await request('POST', `/api/venues/${seg(venue)}/tip-share/reset`);
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
      if (k === 'send.tip_loss' || k === 'send.tip_share') load();
    });
  });

  /** Значение или «—». */
  const v = (x, unit = '') => (x === null || x === undefined ? '—' : `${x}${unit}`);
  const kinds = { LostTip: 'по чаевым', LostSpeed: 'по скорости', Win: 'победа' };
</script>

<h3>Доля чаевых (PGA) — решение 437</h3>
{#if st}
  <p class={st.note ? 'err' : 'muted'}>
    Режим send.tip_mode: {v(st.mode)}{st.note ? ` — не работает: ${st.note}` : ''}.
    {#if st.mode === 'fixed'}Доля не используется (чаевые — send.priority_fee_gwei).{/if}
  </p>
  <table>
    <tbody>
      <tr><td>Доля прибыли на чаевые</td><td>{st.share_pct === null ? 'минимум (send.tip_share_min_pct)' : `${st.share_pct} %`}</td></tr>
      <tr><td>Чаевые последней сделки</td><td>{v(st.last_tip_gwei, ' gwei/газ')}</td></tr>
      <tr><td>Победы / проигрыши по чаевым / по скорости</td><td>{st.wins} / {st.lost_tip} / {st.lost_speed}; побед подряд — {st.streak}</td></tr>
      <tr><td>Последний проигрыш</td><td>
        {#if st.last_loss}
          {kinds[st.last_loss.outcome] ?? st.last_loss.outcome}: наша tx {st.last_loss.ours}, блок {st.last_loss.block},
          наши чаевые {st.last_loss.our_tip_gwei} gwei;
          {st.last_loss.rival ? `перехватчик ${st.last_loss.rival}, его чаевые ${v(st.last_loss.rival_tip_gwei, ' gwei')}` : 'перехватчика по пулам маршрута нет'}
        {:else}—{/if}
      </td></tr>
    </tbody>
  </table>
  <p class="muted">Каждый проигрыш — строка журнала send.tip_loss; доля дошла до максимума — событие send.tip_share_max (Telegram).</p>
  <p>
    <button onclick={load}>Обновить</button>
    <button onclick={reset} disabled={busy}>Сбросить долю к минимуму</button>
  </p>
{/if}
<ErrorText text={error} />
