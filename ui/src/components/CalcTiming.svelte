<!--
  Файл: ui/src/components/CalcTiming.svelte
  Что делает: время расчёта кандидата по этапам (решение 129): строка-итог (всего, очередь до старта, запросы к узлу
    BSC и агрегатору — сколько, ожидание очереди, ответ) и раскрываемая таблица этапов (контекст, бюджет, подбор
    объёма, финальная котировка) с частями (котировка A / B у площадки, стоимость вывода, курсы затрат): время, сколько
    раз, запросы. Этап, на котором расчёт стоял при отказе по сроку, помечен «идёт».
  Кто вызывает: pages/Live.svelte (строка кандидата).
  От чего зависит: ничего (поле timing строки «Кандидатов» — core/calc_report.rs).
  Настройки: нет (срок расчёта profit.calc_timeout_ms — экран «Настройки»).
-->
<script>
  /** timing — отчёт ядра {total_ms, queue_ms, node_*, agg_*, stages: [{name, ms, count, open, node_*, agg_*, parts}]}. */
  let { timing } = $props();

  /** Запросы строки текстом: «узел 3 / 1 / 4 мс»; запросов нет — «—». */
  function calls(r) {
    const out = [];
    if (r.node_calls) out.push(`узел ${r.node_calls}: очередь ${r.node_wait_ms}, ответ ${r.node_ms} мс`);
    if (r.agg_calls) out.push(`агрегатор ${r.agg_calls}: очередь ${r.agg_wait_ms}, ответ ${r.agg_ms} мс`);
    return out.length ? out.join('; ') : '—';
  }
</script>

<details>
  <summary>
    Время расчёта: {timing.total_ms} мс (очередь до старта {timing.queue_ms} мс; запросы — {calls(timing)})
  </summary>
  <table>
    <thead><tr><th>Этап</th><th class="num">мс</th><th class="num">раз</th><th>Запросы</th></tr></thead>
    <tbody>
      {#each timing.stages as s (s.name)}
        <tr>
          <th>{s.name}{#if s.open} <span class="err">идёт</span>{/if}</th>
          <td class="num">{s.ms}</td>
          <td class="num">{s.count}</td>
          <td>{calls(s)}</td>
        </tr>
        {#each s.parts as p (p.name)}
          <tr>
            <td class="part">{p.name}{#if p.open} <span class="err">идёт</span>{/if}</td>
            <td class="num">{p.ms}</td>
            <td class="num">{p.count}</td>
            <td>{calls(p)}</td>
          </tr>
        {/each}
      {/each}
    </tbody>
  </table>
</details>

<style>
  .part { padding-left: 1.5em; }
</style>
