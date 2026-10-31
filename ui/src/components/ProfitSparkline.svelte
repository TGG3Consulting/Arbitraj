<!--
  Файл: ui/src/components/ProfitSparkline.svelte
  Что делает: мини-график прибыли по записям эпизода — простой inline-SVG без сторонних библиотек
    (их в проекте нет). Точки идут по порядку записей (не по реальному времени: записи throttle-нуты
    episode.min_interval_ms и расставлены неравномерно), по оси Y — profit_usd. Рисует линию прибыли,
    нулевую ось (если значения пересекают ноль) и отметку текущей позиции (вертикаль + точка). Навигацию по
    записям даёт плеер (ползунок и шаг) — график только показывает.
  Кто вызывает: components/EpisodeReplay.svelte.
  От чего зависит: ничего.
  Настройки: нет (это показ записанных данных, не настройка системы).
-->
<script>
  /** values — profit_usd по записям (числа); index — текущая запись (отметка на графике). */
  let { values = [], index = 0 } = $props();

  // Система координат SVG: фиксированная сетка, растягивается по ширине через viewBox.
  const W = 600;
  const H = 120;
  const PAD = 8;

  const lo = $derived(values.length ? Math.min(...values) : 0);
  const hi = $derived(values.length ? Math.max(...values) : 0);
  // Координата X записи i; при одной записи — центр.
  const xOf = (i) => (values.length < 2 ? W / 2 : PAD + (i * (W - 2 * PAD)) / (values.length - 1));
  // Координата Y значения v; при нулевом размахе — середина.
  const yOf = (v) => {
    const span = hi - lo;
    if (span === 0) return H / 2;
    return H - PAD - ((v - lo) * (H - 2 * PAD)) / span;
  };

  const line = $derived(values.map((v, i) => `${xOf(i)},${yOf(v)}`).join(' '));
  // Нулевая ось рисуется, только если прибыль и выше, и ниже нуля.
  const zeroY = $derived(lo < 0 && hi > 0 ? yOf(0) : null);
  const cx = $derived(xOf(index));
  const cy = $derived(values.length ? yOf(values[index] ?? 0) : H / 2);
</script>

{#if values.length}
  <svg class="spark" viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img"
    aria-label="График прибыли по записям эпизода">
    {#if zeroY !== null}
      <line class="zero" x1="0" y1={zeroY} x2={W} y2={zeroY} />
    {/if}
    <polyline class="curve" points={line} />
    <line class="cursor" x1={cx} y1="0" x2={cx} y2={H} />
    <circle class="dot" cx={cx} cy={cy} r="3.5" />
  </svg>
  <div class="scale muted">
    <span>макс {hi}</span><span>мин {lo}</span>
  </div>
{:else}
  <p class="muted">Нет записей для графика.</p>
{/if}

<style>
  .spark { width: 100%; height: 120px; display: block; background: var(--bg); border: 1px solid var(--line); border-radius: 8px; }
  .curve { fill: none; stroke: var(--accent); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
  .zero { stroke: var(--line); stroke-width: 1; stroke-dasharray: 4 3; vector-effect: non-scaling-stroke; }
  .cursor { stroke: var(--unset); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .dot { fill: var(--unset); }
  .scale { display: flex; justify-content: space-between; font-size: 12px; margin-top: 2px; }
</style>
