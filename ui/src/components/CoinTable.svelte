<!--
  Файл: ui/src/components/CoinTable.svelte
  Что делает: компактный список экрана «Монеты» (Issue #33): одна строка — одна монета: тикер, адрес коротко,
    итог отбора (прошла / причины коротко), налоги коротко (покупка/продажа/перевод % или «не проверено»,
    «ошибка»), пулы (допущено из всего). Строки не переносятся, прокручивается только таблица.
    Щелчок по строке — onopen(монета): экран открывает всплывающее окно с полной информацией.
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/coinFilter.js, lib/fmt.js (shortAddr).
  Настройки: нет.
-->
<script>
  import { reasons, resultLabel, taxState } from '../lib/coinFilter.js';
  import { shortAddr } from '../lib/fmt.js';

  /** coins — монеты после фильтров; onopen(coin) — открыть окно монеты. */
  let { coins, onopen } = $props();

  /** Налоги коротко. Принимает: CheckState<TokenTaxes>. Возвращает: {text, cls}. */
  function taxShort(t) {
    const s = taxState(t);
    if (s === 'known') return { text: `${t.Known.buy_pct} / ${t.Known.sell_pct} / ${t.Known.transfer_pct} %`, cls: '' };
    if (s === 'failed') return { text: 'ошибка', cls: 'err' };
    return { text: 'не проверено', cls: 'unset' };
  }
  /** Ключ монеты: тикер + адрес (одинаковый тикер бывает разными токенами). */
  const key = (c) => `${c.asset.ticker}|${c.asset.bsc_contract}`;
</script>

<div class="tscroll">
  <table class="compact">
    <thead>
      <tr>
        <th>Монета</th><th>Адрес</th><th>Итог отбора</th>
        <th title="покупка / продажа / перевод">Налоги</th><th class="num" title="допущено / всего">Пулы</th>
      </tr>
    </thead>
    <tbody>
      {#each coins as c (key(c))}
        {@const rs = reasons(c)}
        {@const tx = taxShort(c.taxes)}
        <tr class="click" onclick={() => onopen(c)} title="Подробно: пулы, налоги, причины по площадкам">
          <td><strong>{c.asset.ticker}</strong>{#if c.block && c.block.reason !== 'NotChecked'} <span class="err" title="в чёрном списке">⛔</span>{/if}</td>
          <td><code>{shortAddr(c.asset.bsc_contract) || '—'}</code></td>
          <td class={rs[0] === 'passed' ? 'ok' : 'err'} title={rs.map(resultLabel).join(', ')}>
            {resultLabel(rs[0])}{#if rs.length > 1} <span class="muted">+{rs.length - 1}</span>{/if}
          </td>
          <td class={tx.cls} title={c.taxes === 'NotChecked' && c.tax_wait ? `ждёт: ${c.tax_wait}` : null}>{tx.text}</td>
          <td class="num">{c.pools.filter((p) => p.admission == null).length} / {c.pools.length}</td>
        </tr>
      {:else}
        <tr><td colspan="5" class="muted">Ничего не найдено.</td></tr>
      {/each}
    </tbody>
  </table>
</div>
