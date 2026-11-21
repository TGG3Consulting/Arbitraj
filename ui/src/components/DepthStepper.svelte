<!--
  Файл: ui/src/components/DepthStepper.svelte
  Что делает: стакан всей глубины на странице окна (дополнение решения 125) — шаг по кадрам изменений стакана и снимков
    (‹ › и ползунок): стакан ПОСЛЕ кадра = полный стакан + применённые по порядку изменения (собирает сервер,
    core/window_book.rs, разбор — адаптер биржи); что сделано с кадром (полный, применено, устарело, разрыв версий —
    недостоверен до полного, ждёт полного), версия, сколько уровней всего и сколько изменилось, время биржи и получения,
    верхние уровни (сколько — «Уровней стакана» на странице окна).
  Кто вызывает: components/WindowView.svelte.
  От чего зависит: lib/windowLabels.js.
  Настройки: нет.
-->
<script>
  import { msTime, nsTime } from '../lib/windowLabels.js';

  /** frames — кадры окна с полем book (изменения и снимки). */
  let { frames } = $props();
  let step = $state(0);

  const cur = $derived(frames[Math.min(step, frames.length - 1)]);
  const d = $derived(cur?.decoded?.Ok);
</script>

{#if cur}
  <div class="row">
    <button disabled={step <= 0} onclick={() => (step -= 1)}>‹</button>
    <input type="range" min="0" max={frames.length - 1} bind:value={step} />
    <button disabled={step >= frames.length - 1} onclick={() => (step += 1)}>›</button>
    <span>
      кадр {cur.i + 1} · <code>{cur.venue}</code> · {cur.stream === 'Snapshot' ? 'снимок REST' : d?.full ? 'полный стакан' : 'изменения'}
      · биржа {msTime(d?.exchange_ms)} · получен {nsTime(cur.recv_ns)}
    </span>
  </div>
  {#if cur.decoded?.Err}<p class="err">{cur.decoded.Err}</p>{/if}
  <p class:warn-text={!cur.book.ok}>
    {cur.book.note} · версия {cur.book.version ?? '—'}{#if d} (кадр {d.from ?? '—'}…{d.to ?? '—'}, изменилось уровней
      {d.changed?.[0]} / {d.changed?.[1]}){/if} · всего уровней {cur.book.depth[0]} / {cur.book.depth[1]}
  </p>
  <div class="row">
    {#each ['bids', 'asks'] as side (side)}
      <table class="compact">
        <thead><tr><th colspan="2">{side === 'bids' ? 'Покупка (bids)' : 'Продажа (asks)'}</th></tr><tr><th class="num">Цена</th><th class="num">Количество</th></tr></thead>
        <tbody>{#each cur.book[side] as l, j (j)}<tr><td class="num">{l.price}</td><td class="num">{l.qty}</td></tr>{/each}</tbody>
      </table>
    {/each}
  </div>
{:else}<p class="muted">Кадров изменений стакана в прочитанной части нет.</p>{/if}
