<!--
  Файл: ui/src/components/DexGraph.svelte
  Что делает: блок «Граф маршрутов (фон)» площадки DEX на экране «Площадки» (решение 308): режим dex.graph_background,
    снимок графа (версия снимка и реестра, собран с нуля или накопительно, возраст, рёбер), отставание от реестра сейчас
    и наибольшее за час, изменённых пулов ждут круга, последний накопительный круг (мс, пулов оценено, пар пересобрано,
    отставание при публикации), последняя полная пересборка со сверкой (когда, почему, мс, отличий, итог), расхождения
    самопроверки (последнее — текстом), срабатывания предохранителя и ступени «снимок на прежних настройках» за час и
    сутки, перезапуски фоновой задачи; кнопка «Пересобрать граф» (полная пересборка со сверкой; котировки идут на
    действующем снимке). Перечитывается при открытии, по кнопке «Обновить», после «Пересобрать граф» и по записям журнала
    dex.graph / dex.graph_build (живой поток) — без таймеров.
  Кто вызывает: pages/Venues.svelte (у DEX).
  От чего зависит: lib/api.js (get, request, seg), lib/live.svelte.js (onLive), components/ErrorText.svelte.
  Настройки: dex.graph_background, dex.graph_max_lag_ms, dex.graph_hang_ms, dex.graph_retry_pause_s, dex.graph_verify_s,
    dex.graph_alert_min_s (показ имён; задаются
    ниже, в «Настройках площадки», раздел «DEX»).
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
  let note = $state('');

  /** Что делает: читает состояние GET /api/venues/{id}/dex-graph. Ошибки — в error («не применимо» — пусто). */
  async function load() {
    try {
      st = await get(`/api/venues/${seg(venue)}/dex-graph`);
      error = '';
    } catch (e) {
      if (e.aborted) return;
      st = null;
      error = e.status === 400 ? '' : e.message;
    }
  }

  /** Что делает: «Пересобрать граф» — POST …/dex-graph/rebuild, затем новое состояние. Ошибки — в error. */
  async function rebuild() {
    busy = true;
    note = '';
    try {
      st = await request('POST', `/api/venues/${seg(venue)}/dex-graph/rebuild`);
      error = '';
      note = 'Полная пересборка запрошена — котировки идут на действующем снимке до замены.';
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
      if (k === 'dex.graph' || k === 'dex.graph_build') load();
    });
  });

  /** Миллисекунды текстом или «—». */
  const ms = (v) => (v === null || v === undefined ? '—' : `${Math.round(v)} мс`);
  /** Время UTC текстом или «—». */
  const at = (v) => (v ? new Date(v).toISOString().replace('T', ' ').slice(0, 19) + ' UTC' : '—');
</script>

<h3>Граф маршрутов (фон) — решение 308</h3>
{#if st}
  <p class={st.enabled ? 'ok' : 'muted'}>
    {st.enabled ? 'Включён (dex.graph_background = да): граф строит фоновая задача, расчёт берёт готовый снимок.' : 'Выключен (dex.graph_background не «да»): граф строит сам расчёт, как раньше.'}
    {#if st.enabled && !st.running}Фоновой задачи нет — DEX не подписан на рынок.{/if}
  </p>
  {#if st.enabled}
    {#if !st.ready}<p class="err">Снимка нет{st.why_none ? `: ${st.why_none}` : ' — собирается'}. Котировки по классике.</p>{/if}
    <table>
      <tbody>
        <tr><td>Снимок</td><td>{st.ready ? `версия ${st.snapshot_version} (реестр — ${st.registry_version}), №${st.seq}, ${st.snapshot_full ? 'собран с нуля' : 'накопительный'}, возраст ${ms(st.age_ms)}, рёбер ${st.edges}` : '—'}</td></tr>
        <tr><td>Отставание от реестра</td><td>сейчас {st.lag_ms === null ? 'нет' : ms(st.lag_ms)}; наибольшее, которое видел расчёт, за час — {ms(st.reader_lag_max_hour_ms)} (по нему задаётся dex.graph_max_lag_ms); наибольшее при публикации круга за час — {ms(st.publish_lag_max_hour_ms)}; ждут круга: {st.pending}</td></tr>
        <tr><td>Последний круг</td><td>{st.last_round ? `${at(st.last_round.at)}: ${ms(st.last_round.ms)}, пулов оценено ${st.last_round.evaluated} (изменённых ${st.last_round.changed}, зависимых по курсу ${st.last_round.evaluated - st.last_round.changed}), пар пересобрано ${st.last_round.pairs}, рёбер ${st.last_round.edges}, отставание ${ms(st.last_round.lag_ms)}, ждали ${st.last_round.pending_after}` : '—'}</td></tr>
        <tr><td>Полная пересборка</td><td>{st.last_full ? `${at(st.last_full.at)} — ${st.last_full.reason}: ${ms(st.last_full.ms)} (сборка ${ms(st.last_full.build_ms)}, доводка ${ms(st.last_full.reconcile_ms)}), пулов ${st.last_full.pools}, рёбер ${st.last_full.edges}, ${st.last_full.compared ? `отличий сверки ${st.last_full.differences}` : 'сравнивать было не с чем'}; итог: ${st.last_full.outcome}` : '—'}</td></tr>
        <tr><td>Расхождений самопроверки</td><td class={st.mismatches ? 'err' : ''}>{st.mismatches}{st.last_mismatch_at ? ` (последнее ${at(st.last_mismatch_at)})` : ''}</td></tr>
        <tr><td>Предохранитель (dex.graph_max_lag_ms)</td><td class={st.fuse_hour ? 'err' : ''}>эпизодов: за час {st.fuse_episodes_hour}, за сутки {st.fuse_episodes_day}; котировок без маршрута графа: за час {st.fuse_hour}, за сутки {st.fuse_day} — в штатной работе должно быть 0</td></tr>
        <tr><td>Снимок без накопительного обновления (прежние настройки, догоняющая сборка, доводка)</td><td>выдач котировкам: за час {st.stale_settings_hour}, за сутки {st.stale_settings_day}</td></tr>
        <tr><td>Перезапусков фоновой задачи</td><td class={st.restarts ? 'err' : ''}>{st.restarts}{st.last_restart ? ` — ${st.last_restart}` : ''}</td></tr>
      </tbody>
    </table>
    {#if st.last_mismatch}<details><summary>Отличия последней сверки</summary><pre>{st.last_mismatch}</pre></details>{/if}
  {/if}
  <p>
    <button onclick={load}>Обновить</button>
    <button onclick={rebuild} disabled={busy || !st.enabled || !st.running}>Пересобрать граф</button>
  </p>
  {#if note}<p class="muted">{note}</p>{/if}
{/if}
<ErrorText text={error} />
