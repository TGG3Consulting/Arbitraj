<!--
  Файл: ui/src/components/CostLines.svelte
  Что делает: таблица затрат расчёта кандидата (решение 35): вид, где возникла, сумма и валюта, в USDT,
    учтена ли в цене или в количестве (тогда повторно не вычитается).
  Кто вызывает: pages/Live.svelte, pages/Deals.svelte.
  От чего зависит: ничего.
  Настройки: нет.
-->
<script>
  /** lines — CostLine[] из расчёта ядра (core/costs.rs). */
  let { lines } = $props();
</script>

<table>
  <thead><tr><th>Затрата</th><th>Где</th><th class="num">Сумма</th><th class="num">USDT</th><th>Учёт</th></tr></thead>
  <tbody>
    {#each lines as l, i (i)}
      <tr class:off={l.in_price || l.in_qty}>
        <td><code>{l.kind}</code></td>
        <td>{l.stage}</td>
        <td class="num">{l.amount} {l.asset}</td>
        <td class="num">{l.usdt}</td>
        <td>{l.in_price ? 'в цене' : l.in_qty ? 'в количестве' : 'вычитается'}</td>
      </tr>
    {/each}
  </tbody>
</table>
