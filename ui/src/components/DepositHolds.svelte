<!--
  Файл: ui/src/components/DepositHolds.svelte
  Что делает: решение 413 (t198) — удержание бирж при зачислении монеты в карточке монеты: по каждой бирже, где монета
    есть, — мера (наибольший % последних выборок, transfer.deposit_hold_samples), выборки (записи депозитов, сколько
    зачислено на счёт, %, когда, сделки) или «не измерено» и что тогда берёт расчёт прибыли (налог перевода токена —
    консервативно; налога нет — 0). Ревью t198 (С2): кнопка «Сбросить замер» по бирже (POST /api/coins/deposit-hold/reset;
    журнал core.credit_fact). Перед любой продажей и выводом монеты сделки с биржи бот читает факт баланса.
  Кто вызывает: components/CoinRow.svelte.
  От чего зависит: lib/api.js (post), lib/labels.js (showTime). Данные — поле deposit_holds ответа GET /api/coins
    (core/deposit_hold.rs).
  Настройки: transfer.deposit_hold_samples (экран «Настройки»).
-->
<script>
  import { post } from '../lib/api.js';
  import { showTime } from '../lib/labels.js';

  /** coin — элемент ответа GET /api/coins. */
  let { coin } = $props();
  let result = $state(null);
  let busy = $state(false);

  /** Биржи монеты (presence с сетью — у DEX сети нет). */
  const cex = $derived([...new Set((coin.presence ?? []).filter((p) => p.network).map((p) => p.venue))]);
  /** Налог перевода токена, % (null — не измерен). */
  const transfer = $derived(coin.taxes?.Known ? Number(coin.taxes.Known.transfer_pct) : null);
  /** Замер по бирже. */
  const holdOf = (v) => (coin.deposit_holds ?? []).find((h) => h.venue === v);

  /** «Сбросить замер» по бирже. Итог — под списком; экран обновится по живым данным монеты. */
  async function reset(venue) {
    if (!confirm(`Сбросить замер удержания ${coin.asset.ticker} на ${venue}?`)) return;
    busy = true;
    try {
      result = await post('/api/coins/deposit-hold/reset', { venue, asset: coin.asset });
    } catch (e) {
      result = { ok: false, details: e.message };
    }
    busy = false;
  }
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
            <span class="muted">— наибольший из {h.samples.length} последних замеров</span>
            <button class="quiet" disabled={busy} onclick={() => reset(v)}>Сбросить замер</button>
            <ul class="samples">
              {#each h.samples as s, i (i)}
                <li class="muted">{showTime(s.at)}: запись {s.records}, зачислено на счёт {s.credited} — {Number(s.pct).toFixed(2)}%; сделки: {s.deals.join(', ')}</li>
              {/each}
            </ul>
          {:else if transfer === null}
            {v}: <span class="muted">не измерено; налог токена не измерен — в расчёте 0</span>
          {:else if transfer > 0}
            {v}: <span class="warn-text">не измерено — в расчёте как налог перевода {transfer}% (консервативно, до первого замера)</span>
          {:else}
            {v}: <span class="muted">не измерено — налога токена нет, в расчёте 0</span>
          {/if}
        </li>
      {/each}
    </ul>
    {#if result}<span class={result.ok ? 'ok' : 'err'} role="status">{result.details}</span>{/if}
  </div>
{/if}

<style>
  .holds { margin: 2px 0 8px; padding-left: 20px; }
  .samples { margin: 0; padding-left: 16px; }
</style>
