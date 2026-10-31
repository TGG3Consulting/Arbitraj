<!--
  Файл: ui/src/components/EpisodeReplay.svelte
  Что делает: плеер офлайн-реплея записанного эпизода (решение 74). Переигрывает записи пересчёта (type:"calc")
    из файла эпизода: таймлайн-ползунок по записям, play/пауза, шаг назад/вперёд, выбор скорости (кадров/с,
    запоминается в браузере — lib/prefs.js). Для текущего кадра синхронно показывает: прибыль/проценты/объём и
    средние цены, стаканы ОБЕИХ сторон (верхние уровни bids/asks), состояние пулов маршрута (family, вид, блок +
    полный снимок в «подробнее»), карточку would-be сделки (SignalCard) и мини-график прибыли (ProfitSparkline).
    Это ВОСПРОИЗВЕДЕНИЕ записанных данных, не пересимуляция: узла и живых фидов нет; в записи — верхние уровни
    стакана и верхнее состояние пулов (карт тиков нет), поэтому это записанные решения/рынок, а не расчёт с нуля.
    Сколько записей читать — задаёт зритель (как в EpisodeView). Данные — GET /api/episodes/{id}?limit=N.
  Кто вызывает: pages/Episodes.svelte.
  От чего зависит: lib/api.js, lib/prefs.js, lib/labels.js, components/Modal.svelte, PageSize.svelte,
    ErrorText.svelte, SignalCard.svelte, ProfitSparkline.svelte.
  Настройки: нет (скорость реплея — удобство просмотра в localStorage, не настройка системы).
-->
<script>
  import { untrack } from 'svelte';
  import { get, seg } from '../lib/api.js';
  import { showTime, poolKindLabel } from '../lib/labels.js';
  import { load, save } from '../lib/prefs.js';
  import Modal from './Modal.svelte';
  import PageSize from './PageSize.svelte';
  import ErrorText from './ErrorText.svelte';
  import SignalCard from './SignalCard.svelte';
  import ProfitSparkline from './ProfitSparkline.svelte';

  let { episode, onclose } = $props();

  // Скорость реплея в кадрах/с — это удобство показа, не настройка движка (хранится в браузере зрителя).
  const SPEEDS = [1, 2, 5, 10, 25];
  const PREF_FPS = 'episode.replay_fps';

  let limit = $state(null);
  let data = $state(null);
  let error = $state('');
  let index = $state(0);
  let playing = $state(false);
  let fps = $state(Number(load(PREF_FPS)) || SPEEDS[1]);

  const calcs = $derived((data?.records ?? []).filter((r) => r.type === 'calc'));
  const frame = $derived(calcs[index] ?? null);
  // Прибыль по записям для мини-графика (строки Decimal → числа; нечисловое → 0).
  const profits = $derived(calcs.map((r) => Number(r.profit_usd) || 0));
  const last = $derived(Math.max(0, calcs.length - 1));

  // Чтение файла эпизода — как в EpisodeView; при новом объёме плеер встаёт в начало и на паузу.
  $effect(() => {
    if (!limit) return;
    get(`/api/episodes/${seg(episode.id)}?limit=${limit}`)
      .then((r) => {
        data = r;
        error = '';
        index = 0;
        playing = false;
      })
      .catch((e) => (error = e.message));
  });

  // Проигрывание: пока playing — шаг вперёд раз в 1000/fps мс; на последней записи встаём на паузу.
  $effect(() => {
    if (!playing || calcs.length < 2) return;
    const period = Math.max(20, Math.round(1000 / fps));
    const t = setInterval(() => {
      index = untrack(() => index);
      if (index >= last) {
        playing = false;
      } else {
        index += 1;
      }
    }, period);
    return () => clearInterval(t);
  });

  /** Сменить скорость и запомнить в браузере. Принимает: кадров/с. */
  function setFps(v) {
    fps = v;
    save(PREF_FPS, String(v));
  }
  const step = (d) => {
    playing = false;
    index = Math.max(0, Math.min(last, index + d));
  };
  const toggle = () => {
    if (index >= last) index = 0;
    playing = !playing;
  };
  // Верхние уровни стороны стакана: [{price, qty}].
  const levels = (b, side) => b?.[side] ?? [];
</script>

<Modal title={`Реплей эпизода ${episode.id}`} {onclose}>
  <p class="banner warn offline" role="status">
    Офлайн-реплей ЗАПИСАННЫХ данных: без узла BSC и живых фидов. В записи — верхние уровни стаканов и верхнее
    состояние пулов (карт тиков нет), поэтому это воспроизведение записанных решений и рынка, а не пересимуляция с нуля.
  </p>
  <div class="row">
    <PageSize key="episode.records" bind:value={limit} label="Записей читать:" />
    <a href={`/api/episodes/${seg(episode.id)}/file`} download>Скачать файл</a>
  </div>
  {#if !limit}<p class="unset">Задайте, сколько записей читать.</p>{/if}
  <ErrorText text={error} />
  {#if data?.truncated}<p class="warn-text">Показаны первые {limit} строк файла.</p>{/if}

  {#if calcs.length}
    <ProfitSparkline values={profits} {index} />

    <div class="controls">
      <button class="quiet" onclick={() => step(-1)} disabled={index <= 0} title="Шаг назад">◂</button>
      <button onclick={toggle} disabled={calcs.length < 2}>{playing ? '❚❚ Пауза' : '▶ Пуск'}</button>
      <button class="quiet" onclick={() => step(1)} disabled={index >= last} title="Шаг вперёд">▸</button>
      <input class="slider" type="range" min="0" max={last} bind:value={index}
        oninput={() => (playing = false)} aria-label="Таймлайн записей" />
      <span class="muted">запись {index + 1} из {calcs.length}</span>
      <label class="inline">Скорость:
        <select value={fps} onchange={(e) => setFps(Number(e.currentTarget.value))}>
          {#each SPEEDS as s (s)}<option value={s}>{s} кадр/с</option>{/each}
        </select>
      </label>
    </div>

    {#if frame}
      <div class="metrics">
        <span>Время: <strong>{showTime(frame.t)}</strong></span>
        <span>Объём: <strong>{frame.qty}</strong></span>
        <span>Прибыль: <strong>{frame.profit_usd} USDT</strong> ({frame.profit_pct}%)</span>
        <span>Купить по {frame.buy_avg} · продать по {frame.sell_avg}</span>
      </div>

      <div class="books">
        {#each frame.books as b, j (j)}
          <div class="book">
            <h4><code>{b.venue}</code></h4>
            <table class="compact">
              <thead><tr><th class="num">bid цена</th><th class="num">объём</th><th class="num">ask цена</th><th class="num">объём</th></tr></thead>
              <tbody>
                {#each Array(Math.max(levels(b, 'bids').length, levels(b, 'asks').length)) as _, k (k)}
                  <tr>
                    <td class="num">{levels(b, 'bids')[k]?.price ?? ''}</td>
                    <td class="num">{levels(b, 'bids')[k]?.qty ?? ''}</td>
                    <td class="num">{levels(b, 'asks')[k]?.price ?? ''}</td>
                    <td class="num">{levels(b, 'asks')[k]?.qty ?? ''}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/each}
      </div>

      {#if frame.pools.length}
        <h4>Пулы маршрута</h4>
        {#each frame.pools as p, j (j)}
          <details>
            <summary><code>{p.venue}</code> {p.family} {poolKindLabel(p.kind)} · блок {p.block}</summary>
            <pre>{JSON.stringify(p, null, 1)}</pre>
          </details>
        {/each}
      {/if}

      <h4>Карточка would-be сделки кадра</h4>
      {#if frame.card}<SignalCard card={frame.card} />{:else}<p class="muted">В этой записи карточки нет.</p>{/if}
    {/if}
  {:else if limit && !error}
    <p class="muted">В эпизоде нет записей пересчёта.</p>
  {/if}
</Modal>

<style>
  .offline { border: 1px solid var(--line); border-radius: 8px; margin: 8px 0; }
  .controls { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin: 10px 0; }
  .controls .slider { flex: 1 1 200px; min-width: 160px; }
  .metrics { display: flex; flex-wrap: wrap; gap: 6px 18px; margin: 8px 0; }
  .books { display: flex; flex-wrap: wrap; gap: 16px; }
  .book { flex: 1 1 280px; min-width: 260px; }
  .book h4 { margin: 8px 0 2px; }
</style>
