<!--
  Файл: ui/src/components/DepositHolds.svelte
  Что делает: решение 413 (t198) — удержание бирж при зачислении монеты в карточке монеты: по каждой бирже, где монета
    есть, — замер (сколько записали депозитов, сколько зачислили на счёт, %, когда, сделки замера) или «не измерено» и
    что тогда берёт расчёт прибыли (налог перевода токена — консервативно; налога нет — 0).
  Кто вызывает: components/CoinRow.svelte.
  От чего зависит: lib/labels.js (showTime). Данные — поле deposit_holds ответа GET /api/coins (core/deposit_hold.rs).
  Настройки: нет (замер — данные учёта ядра, не настройка).
-->
<script>
  import { showTime } from '../lib/labels.js';

  /** coin — элемент ответа GET /api/coins. */
  let { coin } = $props();

  /** Биржи монеты (presence с сетью — у DEX сети нет). */
  const cex = $derived([...new Set((coin.presence ?? []).filter((p) => p.network).map((p) => p.venue))]);
  /** Налог перевода токена, % (null — не измерен). */
  const transfer = $derived(coin.taxes?.Known ? Number(coin.taxes.Known.transfer_pct) : null);
  /** Замер по бирже. */
  const holdOf = (v) => (coin.deposit_holds ?? []).find((h) => h.venue === v);
</script>

{#if cex.length}
  <div>
    <span class="muted">Удержание биржи при зачислении (решение 413):</span>
    <ul class="holds">
      {#each cex as v (v)}
        {@const h = holdOf(v)}
        <li>
          {#if h}
            {v}: <strong class={Number(h.pct) > 0 ? 'warn-text' : ''}>{Number(h.pct).toFixed(2)}%</strong>
            <span class="muted">— записи депозитов {h.records}, зачислено на счёт {h.credited}; замер {showTime(h.at)}; сделки: {h.deals.join(', ')}</span>
          {:else if transfer === null}
            {v}: <span class="muted">не измерено; налог токена не измерен — перед продажей бот читает факт баланса</span>
          {:else if transfer > 0}
            {v}: <span class="warn-text">не измерено — в расчёте как налог перевода {transfer}% (консервативно, до первого замера)</span>
          {:else}
            {v}: <span class="muted">не измерено — налога токена нет, в расчёте 0</span>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  .holds { margin: 2px 0 8px; padding-left: 20px; }
</style>
