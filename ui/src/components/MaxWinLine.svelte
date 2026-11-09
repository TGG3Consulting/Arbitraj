<!--
  Файл: ui/src/components/MaxWinLine.svelte
  Что делает: решение 245 (часть Б) — окно максимального объёма кандидата (серия расчётов «без потолка», где оптимум без
    потолка проходит пороги; core/uncapped_maxwin.rs): начало, конец (или почему неизвестен), длительность (точно / не
    меньше), вложить → прибыль на начале и в пике, сколько окон; рядом — медиана и p75 длительности нашей сделки пары
    бирж (или биржи-источника, core/deal_time.rs) и вывод «успели бы» (core/maxwin_view.rs). Показывается самое
    длинное окно записи.
  Кто вызывает: pages/Uncapped.svelte (запись кандидата), components/WindowView.svelte (направления окна).
  От чего зависит: lib/uncappedLabels.js (money, duration), lib/labels.js (showTime).
  Настройки: нет (uncapped.maxwin_gap_ms, uncapped.deal_time_min_n — экран «Настройки»).
-->
<script>
  import { money, duration } from '../lib/uncappedLabels.js';
  import { showTime } from '../lib/labels.js';

  /** wins — {best, last, n} или null; view — {status, ms, exact, deal_time, deal_time_err, would, why} или null. */
  let { wins, view } = $props();
  const b = $derived(wins?.best ?? null);
  /** Длительность из мс: до минуты — секунды с десятыми, дальше — duration. */
  const sec = (ms) => (ms == null ? '—' : Number(ms) < 6e4 ? `${(Number(ms) / 1000).toFixed(1)} с` : duration(Number(ms) / 1000));
  const would = $derived(view?.would === 'yes' ? 'да' : view?.would === 'no' ? 'нет' : 'нет данных');
  const wouldCls = $derived(view?.would === 'yes' ? 'ok' : view?.would === 'no' ? 'err' : 'muted');
  const by = (s) => (s.by === 'pair' ? 'пара' : 'биржа-источник');
</script>

{#if view}
  <p class="muted">
    Окно максимального объёма:
    {#if b}
      {showTime(b.start_at)} —
      {#if view.status === 'ended'}{showTime(b.end_at)} (перестала быть выгодной){:else if view.status === 'open'}идёт{:else}конец неизвестен: расчётов не было после {showTime(b.last_pass_at)}{/if},
      <strong>{view.exact ? '' : 'не меньше '}{sec(view.ms)}</strong>;
      на начале вложить {money(b.start.invested)} → {money(b.start.profit)} USDT, пик {money(b.peak.invested)} → {money(b.peak.profit)} USDT ({showTime(b.peak_at)}){#if wins.n > 1}; окон {wins.n}{/if}.
    {:else}
      нет.
    {/if}
    Сделка: {#if view.deal_time}медиана {sec(view.deal_time.median_ms)}, p75 {sec(view.deal_time.p75_ms)} ({by(view.deal_time)}, {view.deal_time.n} сделок){:else}{view.deal_time_err}{/if}.
    Успели бы: <strong class={wouldCls}>{would}</strong>{#if view.would !== 'yes' && view.would !== 'no'} <span>({view.why})</span>{/if}
  </p>
{/if}
