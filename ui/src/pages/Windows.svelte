<!--
  Файл: ui/src/pages/Windows.svelte
  Что делает: экран «Окна» (решение 125) — все окна: пока направление монеты проходит расчёт прибыльности (и до конца
    нашей сделки), бот пишет сырые кадры стакана и каждую сделку биржи по этой монете. Строка: время начала, монета и
    направления, длительность (у открытого — идёт), наибольшая прибыль на бюджете и без бюджета, отметки «БЫЛА СДЕЛКА»
    и «плюсовая» / «минусовая», отказы предпроверки сделки (решение 266: «сделки нет ×N (направление): причина», метка
    «баланс» — отказ из-за свободного баланса; полный текст — в подсказке), кадров (потеряно), размер файла; «Открыть» — страница окна (WindowView), «Скачать
    файл» — .jsonl.zst как есть. Фильтры: монета, со сделкой / без, плюсовая / минусовая; по страницам (сервер).
    Почему окна не пишутся (window.enabled = нет, не задана window.*) и сколько места занято — над списком.
    Данные — GET /api/windows?limit&offset&coin&deal&outcome (coin — тикер или адрес, решение 211); живое сообщение window обновляет строку на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/coinNames.svelte.js, lib/live.svelte.js, lib/labels.js, lib/windowLabels.js, components/PageSize.svelte,
    components/ErrorText.svelte, components/WindowView.svelte.
  Настройки: нет (window.* — экран «Настройки», категория «Окна»). Сколько строк на странице — задаёт зритель.
-->
<script>
  import { uniqBy } from '../lib/uniq.js';
  import { get, seg } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import { flight } from '../lib/flight.js';
  import { showTime } from '../lib/labels.js';
  import { outcomeMark, winSecs, size, winDirs, winRejects } from '../lib/windowLabels.js';
  import { coinName } from '../lib/coinNames.svelte.js';
  import PageSize from '../components/PageSize.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import WindowView from '../components/WindowView.svelte';

  let limit = $state(null);
  let page = $state(1);
  let coin = $state('');
  let deal = $state('');
  let outcome = $state('');
  let rows = $state([]);
  let total = $state(0);
  let off = $state(null);
  let used = $state(0);
  let error = $state('');
  let open = $state(null);
  let now = $state(Date.now());
  const pages = $derived(limit ? Math.max(1, Math.ceil(total / limit)) : 1);

  /** Перечитывает страницу (signal — отмена). Ошибка — в error; отмена — ничего. */
  async function read(signal) {
    if (!limit) return;
    const q = new URLSearchParams({ limit, offset: (page - 1) * limit });
    if (coin.trim()) q.set('coin', coin.trim());
    if (deal) q.set('deal', deal);
    if (outcome) q.set('outcome', outcome);
    try {
      const r = await get(`/api/windows?${q}`, signal);
      rows = r.windows;
      total = r.total;
      off = r.off;
      used = r.used_bytes;
      error = '';
    } catch (e) {
      if (!e.aborted) error = e.message;
    }
  }
  /** Один запрос в полёте (lib/flight.js); смена фильтров или уход со страницы — отмена прежнего. */
  const reload = flight(read);

  $effect(() => {
    // Любой фильтр или размер страницы — заново с первой страницы.
    void [coin, deal, outcome, limit];
    page = 1;
  });
  $effect(() => {
    void [page, coin, deal, outcome, limit];
    reload();
    return () => reload.cancel();
  });
  $effect(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    const stop = onLive((msg) => {
      if (msg.type === 'window') {
        const i = rows.findIndex((r) => r.id === msg.data.id);
        if (i >= 0) rows[i] = { ...msg.data, bytes: msg.data.bytes ?? rows[i].bytes };
        else if (page === 1 && !coin && !deal && !outcome) rows = [msg.data, ...rows].slice(0, limit ?? rows.length + 1);
      } else if (lagFor(msg, 'core')) reload();
    });
    return () => {
      clearInterval(t);
      stop();
    };
  });
</script>

<h1>Окна</h1>
<p class="muted">Пока направление монеты проходит расчёт прибыльности (и до конца нашей сделки) — сырые кадры стакана и
  каждая сделка биржи по этой монете, как их прислала биржа, со временем биржи и временем получения.</p>
<div class="row">
  <PageSize key="windows.limit" bind:value={limit} label="Окон на странице:" />
  <label class="inline">Монета <input bind:value={coin} placeholder="тикер или 0x…" size="12" /></label>
  <label class="inline">Сделка
    <select bind:value={deal}><option value="">все</option><option value="yes">была</option><option value="no">не было</option></select>
  </label>
  <label class="inline">Итог
    <select bind:value={outcome}><option value="">все</option><option value="plus">плюсовая</option><option value="minus">минусовая</option></select>
  </label>
  <button class="quiet" onclick={reload}>Обновить</button>
  <span class="muted">Занято файлами: {size(used)}</span>
</div>
{#if !limit}<p class="unset">Задайте, сколько окон показывать на странице.</p>{/if}
{#if off}<p class="warn-text">Окна не пишутся: {off}</p>{/if}
<ErrorText text={error} />

<div class="tscroll">
  <table class="compact">
    <thead>
      <tr><th>Начало</th><th>Монета и направления</th><th class="num">Длительность, с</th><th class="num">Макс. прибыль</th>
        <th class="num">Без бюджета</th><th>Отметки</th><th class="num">Кадров</th><th class="num">Размер</th><th></th></tr>
    </thead>
    <tbody>
      {#each uniqBy(rows, (w) => w.id) as w (w.id)}
        {@const m = outcomeMark(w.outcome)}
        <tr>
          <td>{showTime(w.started_at)}</td>
          <td><b>{coinName(w.asset)}</b> {winDirs(w)}</td>
          <td class="num">{winSecs(w, now)}</td>
          <td class="num">{w.max_profit}</td>
          <td class="num">{w.max_profit_unbounded ?? '—'}</td>
          <td>
            {#if !w.ended_at}<span class="ok">открыто</span>{/if}
            {#if w.dry}<span class="tag">сухой</span>{/if}
            {#if w.deals?.length}<span class="tag"><b>БЫЛА СДЕЛКА</b></span>{/if}
            {#if m}<span class={`tag ${m.cls}`}>{m.text}</span>{/if}
            {#if w.stopped}<div class="warn-text">{w.stopped}</div>{/if}
            {#each winRejects(w) as r (r.key)}
              <div class="muted" title={r.full}>сделки нет ×{r.count} ({r.dir}){#if r.balance} <span class="tag">баланс</span>{/if}: {r.text}</div>
            {/each}
          </td>
          <td class="num">{w.frames}{#if w.lost}<div class="warn-text">потеряно {w.lost}</div>{/if}</td>
          <td class="num">{size(w.bytes)}</td>
          <td>
            <button class="quiet" onclick={() => (open = w)}>Открыть</button>
            <a href={`/api/windows/${seg(w.id)}/file`} download>Скачать файл</a>
          </td>
        </tr>
      {:else}
        <tr><td colspan="9" class="muted">Окон нет.</td></tr>
      {/each}
    </tbody>
  </table>
</div>
{#if limit && total > limit}
  <div class="row">
    <button disabled={page <= 1} onclick={() => (page -= 1)}>‹</button>
    <span>страница {page} из {pages} (окон: {total})</span>
    <button disabled={page >= pages} onclick={() => (page += 1)}>›</button>
  </div>
{/if}

{#if open}<WindowView win={open} onclose={() => (open = null)} />{/if}
