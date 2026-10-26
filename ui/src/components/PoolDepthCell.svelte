<!--
  Файл: ui/src/components/PoolDepthCell.svelte
  Что делает: ячейка «Глубина» пула в окне монеты (решение 73): глубина покупки и продажи в полосе цены (USDT) и
    вердикт по каждому направлению (глубокий / мелкий, с гистерезисом), порог dex.min_pool_liquidity_usd, прочитана
    ли полоса целиком, почему глубина не посчитана; спит ли пул (слушается только добавление ликвидности),
    сверка расчёта с квотером по каждой стороне (cross_check_buy / cross_check_sell), подробности площадки (details: tickSpacing). Данные — pools[].depth ответа
    GET /api/coins (шлюз pool_depths).
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
  const checkText = { not_checked: 'не сверена', ok: 'сверена', failed: 'расхождение' };
  /** Сверка стороны с квотером (решение 73: стороны независимо): «покупка: сверена (квотер)». */
  const sideCheck = (name, c) => `${name}: ${checkText[c?.state] ?? c?.state ?? '—'} (квотер)${c?.reason ? ` — ${c.reason}` : ''}`;
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
  <div class={d.cross_check_buy?.state === 'failed' ? 'err' : 'muted'}>{sideCheck('покупка', d.cross_check_buy)}</div>
  <div class={d.cross_check_sell?.state === 'failed' ? 'err' : 'muted'}>{sideCheck('продажа', d.cross_check_sell)}</div>
  {#each d.details ?? [] as x, i (i)}<div class="muted">{x}</div>{/each}
{/if}
