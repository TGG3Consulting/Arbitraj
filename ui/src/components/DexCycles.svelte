<!--
  Файл: ui/src/components/DexCycles.svelte
  Что делает: карточки сигналов циклов DEX ↔ DEX (решение 85, фаза сигнала) на «Живом экране»: по каждой площадке-DEX
    из GET /api/venues/{id}/dex-cycles — базовая монета, вложено и чистая прибыль (через fmt.js), маршрут цикла по шагам
    (пул, вид, комиссия), флаг «исполнимо / нет» + причина. Пусто — направление выключено (dex_dex.enabled) или циклов
    нет. Площадки-DEX берутся из GET /api/venues/kinds (ответ 400 по площадке — не DEX, пропускается). Обновляется на
    старте, по кнопке «Обновить» и при переподключении (lagged); расчёт цикла — read-поверхность, авто-сделок нет.
  Кто вызывает: pages/Live.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/fmt.js, lib/coinLabels.js (poolRefText), lib/labels.js (вид пула),
    components/CopyAddr.svelte, components/NoData.svelte, components/ErrorText.svelte.
  Настройки: нет (dex_dex.* — экран «Настройки»).
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { fmtPrice } from '../lib/fmt.js';
  import { poolRefText } from '../lib/coinLabels.js';
  import { poolKindLabel } from '../lib/labels.js';
  import CopyAddr from './CopyAddr.svelte';
  import NoData from './NoData.svelte';
  import ErrorText from './ErrorText.svelte';

  let cycles = $state([]);
  let error = $state('');
  let note = $state('');

  /** Перечитывает сигналы циклов по всем площадкам-DEX. Ошибка — в error. */
  async function reload() {
    try {
      const venues = await get('/api/venues/kinds');
      const dexes = venues.filter((v) => v.kind === 'Dex');
      const lists = await Promise.all(
        dexes.map(async (v) => {
          try {
            return await get(`/api/venues/${seg(v.id)}/dex-cycles`);
          } catch (e) {
            // 400 — площадка не DEX (гонка списка): пропускаем; прочее — показываем.
            if (e.status !== 400) note = e.message;
            return [];
          }
        }),
      );
      cycles = lists.flat();
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    reload();
    return onLive((msg) => {
      if (msg.type === 'lagged') reload();
    });
  });

  /** Маршрут строкой: «Pancake V3 0x… → Uniswap V3 0x…». */
  const routeText = (c) => c.hops.map((h) => `${poolKindLabel(h.kind)} ${poolRefText(h.pool)}`).join(' → ');
</script>

<div class="row">
  <button onclick={reload}>Обновить</button>
  <span class="muted">Сигнал (решение 85): расчёт без исполнения — авто-сделок по циклам нет.</span>
</div>
<ErrorText text={error} />
<NoData text={note} />

{#each cycles as c, i (`${c.venue}-${i}`)}
  <article class="card" class:hl={c.net_profit != null && Number(c.net_profit) > 0}>
    <header>
      <strong>{c.base_ticker}</strong>
      <CopyAddr addr={c.base} />
      <code>{c.venue}</code>
      {#if c.executable}<span class="tag ok">исполнимо</span>{:else}<span class="tag err">сигнал</span>{/if}
      <span class="muted">блок {c.block}</span>
    </header>
    <p>
      Вложено <strong>{fmtPrice(c.input)}</strong> {c.base_ticker} → получено {fmtPrice(c.output)};
      валовая {fmtPrice(c.gross_profit)}
    </p>
    <p>
      {#if c.net_profit != null}
        Чистая прибыль: <strong class={Number(c.net_profit) > 0 ? 'ok' : 'err'}>{fmtPrice(c.net_profit)} {c.base_ticker}</strong>
        <span class="muted">(затраты {fmtPrice(c.bnb_cost)} BNB ≈ {fmtPrice(c.bnb_cost_in_base)} {c.base_ticker})</span>
      {:else}
        <span class="warn-text">чистая прибыль не посчитана: газ/чаевые нечем перевести в {c.base_ticker} (нет пула база ↔ WBNB)</span>
        <span class="muted">— затраты {fmtPrice(c.bnb_cost)} BNB</span>
      {/if}
    </p>
    {#if !c.executable && c.exec_note}
      <p class="warn-text">Не исполнимо как есть: {c.exec_note}</p>
    {/if}
    <p class="route">Маршрут: {routeText(c)}</p>
    {#each c.hops as h (h.index)}
      <p class="leg muted">{h.index}. {poolKindLabel(h.kind)} <code>{poolRefText(h.pool)}</code> — комиссия {fmtPrice(h.fee_pct)}%{#if Number(h.hook_pct) !== 0}, крючок {fmtPrice(h.hook_pct)}%{/if}, газ {h.gas_units}</p>
    {/each}
  </article>
{:else}
  <p class="muted">Сигналов циклов нет: направление выключено (dex_dex.enabled) или прибыльных циклов не найдено.</p>
{/each}

<style>
  .row { display: flex; gap: 10px; align-items: baseline; margin: 6px 0; }
  .card p { margin: 4px 0; }
  .card p.route { overflow-wrap: anywhere; }
  .card p.leg { margin: 2px 0 2px 16px; font-size: 0.95em; }
  .tag.ok { color: var(--ok); } .tag.err { color: var(--err); }
</style>
