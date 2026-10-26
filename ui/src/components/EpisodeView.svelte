<!--
  Файл: ui/src/components/EpisodeView.svelte
  Что делает: окно просмотра эпизода (решение 74): записи из файла эпизода — по строке на пересчёт: время, объём,
    прибыль на бюджете, максимальный объём и прибыль без бюджета, лучшие цены стаканов, блоки пулов; щелчок по
    строке — карточка сигнала этой записи (SignalCard) и стаканы верхних episode.book_levels уровней. Сколько записей
    читать — задаёт зритель.
  Кто вызывает: pages/Episodes.svelte.
  От чего зависит: lib/api.js, components/Modal.svelte, PageSize.svelte, ErrorText.svelte, SignalCard.svelte, lib/labels.js.
  Настройки: нет.
-->
<script>
  import { get, seg } from '../lib/api.js';
  import { showTime } from '../lib/labels.js';
  import Modal from './Modal.svelte';
  import PageSize from './PageSize.svelte';
  import ErrorText from './ErrorText.svelte';
  import SignalCard from './SignalCard.svelte';

  let { episode, onclose } = $props();
  let limit = $state(null);
  let data = $state(null);
  let error = $state('');
  let pick = $state(null);

  $effect(() => {
    if (!limit) return;
    get(`/api/episodes/${seg(episode.id)}?limit=${limit}`)
      .then((r) => {
        data = r;
        error = '';
      })
      .catch((e) => (error = e.message));
  });

  const calcs = $derived((data?.records ?? []).filter((r) => r.type === 'calc'));
  const best = (b, side) => b[side]?.[0]?.price ?? '—';
</script>

<Modal title={`Эпизод ${episode.id}`} {onclose}>
  <div class="row">
    <PageSize key="episode.records" bind:value={limit} label="Записей читать:" />
    <a href={`/api/episodes/${seg(episode.id)}/file`} download>Скачать файл</a>
  </div>
  {#if !limit}<p class="unset">Задайте, сколько записей читать.</p>{/if}
  <ErrorText text={error} />
  {#if data?.truncated}<p class="warn-text">Показаны первые {limit} строк файла.</p>{/if}
  <div class="tscroll">
    <table class="compact">
      <thead><tr><th>Время</th><th class="num">Объём</th><th class="num">Прибыль</th><th class="num">Макс. объём</th>
        <th class="num">Макс. прибыль</th><th>Стаканы (bid / ask)</th><th>Блоки пулов</th></tr></thead>
      <tbody>
        {#each calcs as r, i (i)}
          <tr class="click" onclick={() => (pick = r)}>
            <td>{showTime(r.t)}</td>
            <td class="num">{r.qty}</td>
            <td class="num">{r.profit_usd}</td>
            <td class="num">{r.card?.max?.Ok?.max_qty ?? '—'}</td>
            <td class="num">{r.card?.max?.Ok?.opt_profit ?? '—'}</td>
            <td>{#each r.books as b, j (j)}<code>{b.venue}</code> {best(b, 'bids')} / {best(b, 'asks')} {/each}</td>
            <td>{r.pools.map((p) => p.block).join(', ') || '—'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if pick}
    <h3>Запись {showTime(pick.t)}</h3>
    <SignalCard card={pick.card} />
    {#each pick.books as b, j (j)}
      <details><summary>Стакан {b.venue}: {b.bids.length} bids, {b.asks.length} asks</summary><pre>{JSON.stringify(b, null, 1)}</pre></details>
    {/each}
    {#each pick.pools as p, j (j)}
      <details><summary>Пул {p.family} блок {p.block}</summary><pre>{JSON.stringify(p, null, 1)}</pre></details>
    {/each}
  {/if}
</Modal>
