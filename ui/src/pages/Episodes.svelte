<!--
  Файл: ui/src/pages/Episodes.svelte
  Что делает: экран «Эпизоды» (решение 74) — эпизоды арбитражной ситуации: монета, направление, сухой ли, начало,
    длительность (у открытого — идёт), наибольшая прибыль на бюджете и без бюджета, записей, причина закрытия или
    «открыт», остановлена ли запись (лимит места); «Просмотр» — записи эпизода в окне (EpisodeView), «Скачать» — файл
    .jsonl.zst как есть. Почему эпизоды не ведутся (не заданы episode.*) и сколько места занято — над списком.
    «Реплей» — плеер офлайн-реплея записанного эпизода (EpisodeReplay).
    Данные — GET /api/episodes?limit=N; живое сообщение episode обновляет строку на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/PageSize.svelte,
    components/ErrorText.svelte, components/EpisodeView.svelte, components/EpisodeReplay.svelte.
  Настройки: нет (episode.* — экран «Настройки», категория «Сигналы и эпизоды»). Сколько строк — задаёт зритель.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { onLive } from '../lib/live.svelte.js';
  import { dirText } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import PageSize from '../components/PageSize.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import EpisodeView from '../components/EpisodeView.svelte';
  import EpisodeReplay from '../components/EpisodeReplay.svelte';

  let limit = $state(null);
  let rows = $state([]);
  let off = $state(null);
  let used = $state(0);
  let error = $state('');
  let open = $state(null);
  let replay = $state(null);
  let now = $state(Date.now());

  /** Перечитывает список. Ошибка — в error. */
  async function reload() {
    if (!limit) return;
    try {
      const r = await get(`/api/episodes?limit=${limit}`);
      rows = r.episodes;
      off = r.off;
      used = r.used_bytes;
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  $effect(() => {
    reload();
    // Длительность открытых эпизодов идёт — раз в секунду пересчитывается только показ.
    const t = setInterval(() => (now = Date.now()), 1000);
    const stop = onLive((msg) => {
      if (msg.type === 'episode') {
        const i = rows.findIndex((r) => r.id === msg.data.id);
        if (i >= 0) rows[i] = msg.data;
        else rows = [msg.data, ...rows];
      } else if (msg.type === 'lagged') reload();
    });
    return () => {
      clearInterval(t);
      stop();
    };
  });

  /** Длительность эпизода, секунд. Принимает: сводку. */
  const secs = (e) => (((e.ended_at ? Date.parse(e.ended_at) : now) - Date.parse(e.started_at)) / 1000).toFixed(1);
</script>

<h1>Эпизоды арбитражной ситуации</h1>
<div class="row">
  <PageSize key="episodes.limit" bind:value={limit} label="Эпизодов в списке:" />
  <button class="quiet" onclick={reload}>Обновить</button>
  <span class="muted">Занято файлами: {(used / 1048576).toFixed(2)} МБ</span>
</div>
{#if !limit}<p class="unset">Задайте, сколько эпизодов показывать.</p>{/if}
{#if off}<p class="warn-text">Эпизоды не ведутся: {off}</p>{/if}
<ErrorText text={error} />

<div class="tscroll">
  <table class="compact">
    <thead>
      <tr><th>Монета и направление</th><th>Режим</th><th>Начало</th><th class="num">Длительность, с</th>
        <th class="num">Макс. прибыль</th><th class="num">Без бюджета</th><th class="num">Записей</th><th>Итог</th><th></th></tr>
    </thead>
    <tbody>
      {#each rows as e (e.id)}
        <tr>
          <td>{dirText(e.direction)}</td>
          <td>{#if e.dry}<span class="tag">сухой</span>{:else}обычный{/if}</td>
          <td>{showTime(e.started_at)}</td>
          <td class="num">{secs(e)}</td>
          <td class="num">{e.max_profit}</td>
          <td class="num">{e.max_profit_unbounded ?? '—'}</td>
          <td class="num">{e.records}</td>
          <td>
            {#if e.ended_at}{e.close_reason}{:else}<span class="ok">открыт</span>{/if}
            {#if e.stopped}<div class="warn-text">{e.stopped}</div>{/if}
          </td>
          <td>
            <button class="quiet" onclick={() => (open = e)}>Просмотр</button>
            <button class="quiet" onclick={() => (replay = e)}>Реплей</button>
            <a href={`/api/episodes/${seg(e.id)}/file`} download>Скачать</a>
          </td>
        </tr>
      {:else}
        <tr><td colspan="9" class="muted">Эпизодов нет.</td></tr>
      {/each}
    </tbody>
  </table>
</div>

{#if open}<EpisodeView episode={open} onclose={() => (open = null)} />{/if}
{#if replay}<EpisodeReplay episode={replay} onclose={() => (replay = null)} />{/if}
