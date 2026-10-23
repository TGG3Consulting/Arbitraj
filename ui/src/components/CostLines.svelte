<!--
  Файл: ui/src/components/CostLines.svelte
  Что делает: затраты расчёта кандидата (решения 35, 52), сгруппированные по шагам маршрута обмена:
    «шаг 1: USDT → BNB», «шаг 2: BNB → монета» — комиссия пула, проскальзывание, газ, налоги каждого шага;
    затраты вне обмена (комиссия биржи, вывод, возврат USDT, запас r) — отдельной группой.
    По строке: вид, где возникла, сумма и валюта, в USDT, учтена ли в цене или в количестве.
  Кто вызывает: pages/Live.svelte, pages/Deals.svelte.
  От чего зависит: ничего.
  Настройки: нет.
-->
<script>
  /** lines — CostLine[] из расчёта ядра (core/costs.rs), у каждой step: {index, from, to} | null. */
  let { lines } = $props();

  /** Группы по шагу: [{title, rows}], шаги по номеру, затраты без шага — в конце. */
  const groups = $derived.by(() => {
    const by = new Map();
    for (const l of lines) {
      const key = l.step ? l.step.index : Infinity;
      if (!by.has(key)) {
        const title = l.step ? `Шаг ${l.step.index}: ${l.step.from} → ${l.step.to}` : 'Вне обмена: биржа, перевод, возврат USDT, запас r';
        by.set(key, { key, title, rows: [] });
      }
      by.get(key).rows.push(l);
    }
    return [...by.values()].sort((a, b) => a.key - b.key);
  });
</script>

<table>
  <thead><tr><th>Затрата</th><th>Где</th><th class="num">Сумма</th><th class="num">USDT</th><th>Учёт</th></tr></thead>
  {#each groups as g (g.key)}
    <tbody>
      <tr><th colspan="5">{g.title}</th></tr>
      {#each g.rows as l, i (i)}
        <tr class:off={l.in_price || l.in_qty}>
          <td><code>{l.kind}</code></td>
          <td>{l.stage}</td>
          <td class="num">{l.amount} {l.asset}</td>
          <td class="num">{l.usdt ?? 'нет живой цены'}</td>
          <td>{l.in_price ? 'в цене' : l.in_qty ? 'в количестве' : 'вычитается'}</td>
        </tr>
      {/each}
    </tbody>
  {/each}
</table>
