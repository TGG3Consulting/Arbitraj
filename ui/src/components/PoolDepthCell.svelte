<!--
  Файл: ui/src/components/PoolDepthCell.svelte
  Что делает: ячейка «Глубина» пула в окне монеты (решение 73): глубина покупки и продажи в полосе цены (USDT) и
    вердикт по каждому направлению (глубокий / мелкий, с гистерезисом), порог dex.min_pool_liquidity_usd, прочитана
    ли полоса целиком, почему глубина не посчитана; спит ли пул (слушается только добавление ликвидности),
    сверка с квотером, tickSpacing. Данные — pools[].depth ответа GET /api/coins (шлюз pool_depths).
  Кто вызывает: components/CoinRow.svelte.
  От чего зависит: ничего.
  Настройки: нет (dex.depth_band_pct, dex.depth_hysteresis_pct, dex.depth_log_min_s, dex.min_pool_liquidity_usd —
    экран «Настройки»).
-->
<script>
  /** p — пул из coin.pools ответа GET /api/coins. */
  let { p } = $props();
  const d = $derived(p.depth);
  /** Строка направления: «покупка 812.40 USDT — мелкий». */
  const side = (name, usd, deep) => `${name} ${usd ?? '—'} USDT — ${deep ? 'глубокий' : 'мелкий'}`;
  const quoterText = { not_checked: 'не сверен с квотером', ok: 'сверен с квотером', failed: 'расхождение с квотером' };
</script>

{#if p.depth_unavailable}
  <span class="muted">нет данных: {p.depth_unavailable}</span>
{:else if !d}
  <span class="muted">—</span>
{:else}
  {#if d.depth_error}
    <span class="err">не считается: {d.depth_error}</span>
  {:else}
    <div class={d.deep_buy ? 'ok' : 'err'}>{side('покупка', d.buy_usd, d.deep_buy)}</div>
    <div class={d.deep_sell ? 'ok' : 'err'}>{side('продажа', d.sell_usd, d.deep_sell)}</div>
    <div class="muted">полоса ±{d.band_pct}% · порог {d.min_usd} USDT{d.full ? '' : ' · полоса прочитана не целиком'}</div>
  {/if}
  {#if d.asleep}<div class="warn-text">спит: слушается только добавление ликвидности</div>{/if}
  {#if !d.valid}<div class="warn-text">данные пула недостоверны</div>{/if}
  <div class={d.quoter === 'failed' ? 'err' : 'muted'}>
    {quoterText[d.quoter] ?? d.quoter}{d.quoter_reason ? `: ${d.quoter_reason}` : ''}
  </div>
  {#if d.tick_spacing != null}<div class="muted">tickSpacing {d.tick_spacing}</div>{/if}
{/if}
