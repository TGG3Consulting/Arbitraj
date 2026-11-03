<!--
  Файл: ui/src/components/WindowView.svelte
  Что делает: страница окна (решение 125): что писалось на каждой бирже (каналы стакана и сделок, вид ленты или
    причина, почему не пишется); стакан по времени — шаг по кадрам стакана (‹ › и ползунок), уровни на момент кадра,
    время биржи и время получения ботом; лента сделок — каждая сделка (время биржи, время получения, цена, количество,
    сторона агрессора, ID) вместе с событиями наших сделок окна на одной шкале времени; «Скачать файл». Кадры читаются
    с offset порциями по limit (задаёт зритель); разбирает их адаптер своей площадки на сервере.
  Кто вызывает: pages/Windows.svelte.
  От чего зависит: lib/api.js, lib/windowLabels.js, components/Modal.svelte, PageSize.svelte, ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { msTime, nsTime, outcomeMark } from '../lib/windowLabels.js';
  import Modal from './Modal.svelte';
  import PageSize from './PageSize.svelte';
  import ErrorText from './ErrorText.svelte';

  let { win, onclose } = $props();
  let limit = $state(null);
  let offset = $state(0);
  let data = $state(null);
  let error = $state('');
  let step = $state(0);
  let levels = $state(null);

  $effect(() => {
    if (!limit) return;
    get(`/api/windows/${seg(win.id)}?limit=${limit}&offset=${offset}`)
      .then((r) => {
        data = r;
        step = 0;
        error = '';
      })
      .catch((e) => (error = e.message));
  });

  const info = $derived(data?.info ?? win);
  const mark = $derived(outcomeMark(info.outcome));
  const books = $derived((data?.frames ?? []).filter((f) => f.stream === 'Book'));
  const cur = $derived(books[Math.min(step, books.length - 1)]);
  const curView = $derived(cur?.decoded?.Ok?.view?.Book);
  const show = (side) => (curView?.[side] ?? []).slice(0, levels ?? undefined);
  /** Лента: сделки кадров и события наших сделок, по времени (мс). */
  const tape = $derived.by(() => {
    const out = [];
    for (const f of data?.frames ?? []) {
      if (f.stream !== 'Trades') continue;
      const ok = f.decoded?.Ok;
      if (!ok) { out.push({ t: f.recv_ns / 1e6, kind: 'err', f, text: f.decoded?.Err ?? '' }); continue; }
      for (const x of ok.view.Trades ?? []) out.push({ t: x.time_ms, kind: 'trade', f, x });
    }
    for (const d of data?.deals ?? []) {
      for (const [at, text] of d.timeline) out.push({ t: Date.parse(at), kind: 'deal', text: `сделка ${d.id}: ${text}` });
    }
    return out.sort((a, b) => a.t - b.t);
  });
</script>

<Modal title={`Окно ${win.id}`} {onclose}>
  <p>
    <b>{info.asset.ticker}</b> · {info.directions.map((d) => `${d.buy_on} → ${d.sell_on}`).join(', ')}
    · макс. прибыль {info.max_profit}{#if info.max_profit_unbounded != null}, без бюджета {info.max_profit_unbounded}{/if}
    {#if info.deals?.length}<span class="tag"><b>БЫЛА СДЕЛКА</b></span>{/if}
    {#if mark}<span class={`tag ${mark.cls}`}>{mark.text}</span>{/if}
    · <a href={`/api/windows/${seg(win.id)}/file`} download>Скачать файл</a>
  </p>
  {#each info.venues as v (v.venue)}
    <div class="muted">
      <code>{v.venue}</code> с {msTime(Date.parse(v.since))}:
      {#if v.tap?.Ok}стакан <code>{v.tap.Ok.book_channel}</code>;
        сделки {#if v.tap.Ok.trades_channel.Ok}<code>{v.tap.Ok.trades_channel.Ok}</code> — {v.tap.Ok.trades_form}{:else}<span class="warn-text">не пишутся: {v.tap.Ok.trades_channel.Err}</span>{/if}
      {:else if v.tap?.Err}<span class="err">запись не включена: {v.tap.Err}</span>
      {:else}<span class="muted">запись включается…</span>{/if}
    </div>
  {/each}
  {#if info.stopped}<p class="warn-text">{info.stopped}</p>{/if}
  {#if info.lost}<p class="warn-text">Потеряно кадров (запись не успевала): {info.lost}</p>{/if}

  <div class="row">
    <PageSize key="window.frames" bind:value={limit} label="Кадров читать:" />
    <button disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - limit))}>‹ раньше</button>
    <span>кадры с {offset + 1}</span>
    <button disabled={!data?.truncated} onclick={() => (offset += limit)}>дальше ›</button>
  </div>
  {#if !limit}<p class="unset">Задайте, сколько кадров читать.</p>{/if}
  <ErrorText text={error} />

  <h3>Стакан по времени ({books.length} кадров)</h3>
  {#if cur}
    <div class="row">
      <button disabled={step <= 0} onclick={() => (step -= 1)}>‹</button>
      <input type="range" min="0" max={books.length - 1} bind:value={step} />
      <button disabled={step >= books.length - 1} onclick={() => (step += 1)}>›</button>
      <span>кадр {cur.i + 1} · <code>{cur.venue}</code> · биржа {msTime(cur.decoded?.Ok?.exchange_ms)} · получен {nsTime(cur.recv_ns)}</span>
      <PageSize key="window.levels" bind:value={levels} label="Уровней:" />
    </div>
    {#if cur.decoded?.Err}<p class="err">{cur.decoded.Err}</p>{/if}
    <div class="row">
      {#each ['bids', 'asks'] as side (side)}
        <table class="compact">
          <thead><tr><th colspan="2">{side === 'bids' ? 'Покупка (bids)' : 'Продажа (asks)'}</th></tr><tr><th class="num">Цена</th><th class="num">Количество</th></tr></thead>
          <tbody>{#each show(side) as l, j (j)}<tr><td class="num">{l.price}</td><td class="num">{l.qty}</td></tr>{/each}</tbody>
        </table>
      {/each}
    </div>
  {:else}<p class="muted">Кадров стакана в прочитанной части нет.</p>{/if}

  <h3>Лента сделок и наши сделки</h3>
  <div class="tscroll">
    <table class="compact">
      <thead><tr><th>Время биржи</th><th>Получен</th><th>Площадка</th><th class="num">Цена</th><th class="num">Количество</th><th>Агрессор</th><th>ID</th></tr></thead>
      <tbody>
        {#each tape as r, j (j)}
          {#if r.kind === 'trade'}
            <tr><td>{msTime(r.x.time_ms)}</td><td>{nsTime(r.f.recv_ns)}</td><td><code>{r.f.venue}</code></td>
              <td class="num">{r.x.price}</td><td class="num">{r.x.qty}</td>
              <td class={r.x.taker === 'buy' ? 'ok' : 'err'}>{r.x.taker === 'buy' ? 'покупка' : r.x.taker === 'sell' ? 'продажа' : r.x.taker}</td><td><code>{r.x.id}</code></td></tr>
          {:else if r.kind === 'deal'}
            <tr><td>{msTime(r.t)}</td><td colspan="6"><b>{r.text}</b></td></tr>
          {:else}
            <tr><td colspan="7" class="err">{nsTime(r.f.recv_ns)} {r.f.venue}: кадр не разобран — {r.text}</td></tr>
          {/if}
        {:else}
          <tr><td colspan="7" class="muted">Сделок в прочитанной части нет.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</Modal>
