<!--
  Файл: ui/src/components/CoinRow.svelte
  Что делает: одна монета на экране «Монеты»: тикер и адрес, площадки, налоги (покупка, продажа, перевод)
    и состояние проверки, источник и время проверки, чёрный список с причиной; пулы монеты — площадка, вид,
    ссылка на пул, комиссия, крючок и его группа A/B/C, проверка влияния крючка, причина исключения пула
    (admission), номер блока состояния (block), пара пула (pair: USDT или промежуточная монета) и через какую
    промежуточную монету допущена монета без пула с USDT (via, решение 52).
    Кнопка «Перепроверить» — по каждой площадке, где у монеты есть пулы (POST /api/coins/recheck).
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/api.js, lib/coinLabels.js, lib/labels.js, components/ErrorText.svelte.
  Настройки: нет (пороги tax.*, hooks.* — экран «Настройки»).
-->
<script>
  import { post } from '../lib/api.js';
  import { assetLabel, checkState, taxesText, impactText, blockReason, hookGroupLabel, poolRefText, exclusionText } from '../lib/coinLabels.js';
  import { poolKindLabel, showTime } from '../lib/labels.js';

  /** coin — элемент ответа GET /api/coins. */
  let { coin } = $props();
  let result = $state(null);
  let busy = $state(false);

  const taxes = $derived(checkState(coin.taxes, taxesText));
  /** Площадки с пулами этой монеты — там есть что перепроверять. */
  const dexVenues = $derived([...new Set(coin.pools.map((p) => p.venue))]);

  /** «Перепроверить» на площадке. Принимает: id площадки. Результат или сбой — под строкой. */
  async function recheck(venue) {
    busy = true;
    try {
      result = await post('/api/coins/recheck', { venue, assets: [coin.asset] });
    } catch (e) {
      result = { ok: false, details: e.message };
    }
    busy = false;
  }

</script>

<article class="card">
  <header>
    <strong>{assetLabel(coin.asset)}</strong>
    <span class="muted">площадки: {coin.venues.join(', ')}</span>
    {#each coin.via ?? [] as v (v.venue)}
      <span class="tag">{v.venue}: маршрут через {v.via} — пула с USDT нет, допущена по решению 52</span>
    {/each}
    {#if coin.block}<span class="err">в чёрном списке: {blockReason(coin.block.reason)} (с {showTime(coin.block.since)})</span>
    {:else}<span class="ok">торгуется</span>{/if}
  </header>
  {#each (coin.presence ?? []).filter((p) => p.excluded.length) as p (p.venue)}
    <p class="muted">Не участвует на {p.venue}: {p.excluded.map(exclusionText).join('; ')}</p>
  {/each}
  <p>
    Налоги: <span class={taxes.cls}>{taxes.text}</span>
    <span class="muted">· источник {coin.tax_source ?? '—'} · {coin.checked_at ? showTime(coin.checked_at) : 'ещё не проверялось'}</span>
  </p>
  {#if coin.pools.length}
    <table>
      <thead><tr><th>Площадка</th><th>Пара</th><th>Вид пула</th><th>Пул</th><th>Комиссия</th><th>Крючок</th><th>Проверка</th><th>Получено</th></tr></thead>
      <tbody>
        {#each coin.pools as p, i (i)}
          {@const chk = p.check ? checkState(p.check.impact, impactText) : null}
          <tr class:off={p.admission}>
            <td>{p.venue}</td>
            <td>{coin.asset.ticker} / {p.pair ?? 'USDT'}</td>
            <td><code>{p.family}</code> {poolKindLabel(p.kind)}</td>
            <td><code>{poolRefText(p.pool)}</code></td>
            <td>{p.fee_pips == null ? 'не из пула (V2 — настройка, V4 — динамическая)' : `${p.fee_pips / 10000}%`}</td>
            <td>{p.hooks ? `${hookGroupLabel(p.hooks.group)}${p.hooks.dynamic_fee ? ', динамическая комиссия' : ''}` : 'нет'}</td>
            <td>
              {#if chk}<span class={chk.cls}>{chk.text}</span>{:else}<span class="muted">не требуется / нет данных</span>{/if}
              {#if p.admission}<div class="err">пул исключён: {p.admission}</div>{/if}
            </td>
            <td>{p.received_at ? showTime(p.received_at) : '—'}{#if p.block != null} <span class="muted">· блок {p.block}</span>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="muted">Пулов у монеты нет.</p>
  {/if}
  <div class="row">
    {#each dexVenues as v (v)}
      <button class="quiet" disabled={busy} onclick={() => recheck(v)}>Перепроверить на {v}</button>
    {/each}
    {#if result}<span class={result.ok ? 'ok' : 'err'} role="status">{result.details}</span>{/if}
  </div>
</article>
