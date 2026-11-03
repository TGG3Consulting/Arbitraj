<!--
  Файл: ui/src/components/DryResult.svelte
  Что делает: итог «сухой сделки DEX» (решения 121 C4, 124) таблицей — общий для свопа монеты и цикла DEX ↔ DEX:
    «годится» или причина отказа, монета и сторона (у цикла — базовая монета и «цикл»), количество (у цикла — вход),
    форма маршрута, пулы, выход по котировке / живому пересчёту / минимум (у цикла — граница ядра) / по симуляции, газ
    обмена и разрешений, откуда вход в симуляции, блок, заметки (у цикла — план ядра и чистая прибыль по симуляции).
  Кто вызывает: components/DexDrySwap.svelte, components/DexDryCycle.svelte.
  От чего зависит: lib/coinLabels.js (poolRefText).
  Настройки: нет.
-->
<script>
  import { poolRefText } from '../lib/coinLabels.js';

  /** Итог POST /api/dex/dry-swap или /api/dex/dry-cycle (DexDrySwap); null — ещё не проверяли. */
  let { res = null, cycle = false } = $props();

  /** Значение или «—» (не дошли до этого шага). */
  const v = (x) => (x === null || x === undefined ? '—' : x);
</script>

{#if res}
  <p class={res.ok ? 'ok' : 'err'} role="status">
    {res.ok ? 'Годится: собрано, симуляция прошла, выход ≥ минимума, обмен сделали пулы маршрута.' : `Отказ: ${res.reason ?? 'без причины'}`}
  </p>
  <table>
    <tbody>
      {#if cycle}
        <tr><th>Цикл</th><td>базовая {res.asset.ticker} <code>{res.asset.bsc_contract}</code>, вход ≈ {res.usdt} USDT</td></tr>
      {:else}
        <tr><th>Монета, сторона</th><td>{res.asset.ticker} <code>{res.asset.bsc_contract}</code>, {res.side === 'Buy' ? 'покупка' : 'продажа'} на {res.usdt} USDT</td></tr>
      {/if}
      <tr><th>{cycle ? 'Вход цикла (оптимальный по плану ядра)' : 'Количество монет'}</th><td>{v(res.qty)}</td></tr>
      <tr><th>Форма маршрута</th><td>{v(res.form)}</td></tr>
      <tr><th>Пулы</th><td>
        {#each res.pools as p, i (i)}<div>доля {p.leg + 1}: {p.dex} ({p.kind}) <code>{poolRefText(p.pool)}</code></div>{:else}—{/each}
      </td></tr>
      <tr><th>Выход по котировке</th><td>{v(res.quote_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Выход по живому пересчёту</th><td>{v(res.live_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>{cycle ? 'Граница ядра (minOut)' : 'Минимум (amountOutMin)'}</th><td>{v(res.min_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Выход по симуляции</th><td>{v(res.sim_out)} {res.out_symbol ?? ''}</td></tr>
      <tr><th>Газ обмена / разрешений</th><td>{v(res.gas_swap)} / {v(res.gas_approve)} (разрешений: {res.approvals})</td></tr>
      <tr><th>Вход в симуляции</th><td>{v(res.funding)}</td></tr>
      <tr><th>Блок симуляции</th><td>{v(res.block)}</td></tr>
    </tbody>
  </table>
  {#each res.notes as n, i (i)}<p class="muted">{n}</p>{/each}
{/if}
