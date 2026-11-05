<!--
  Файл: ui/src/components/Delistings.svelte
  Что делает: блок «Делистинги» площадки на экране «Площадки» (решение 175): когда получен последний ответ биржи,
    список пар в делистинге (монета, символ, когда прекращаются торги и закрывается вывод, источник в API) или
    «неизвестно» с причиной (у биржи нет источника в официальном API — сделки по делистингу не блокируются), ошибка
    последнего запроса. Порог «не торговать за N часов» — delist.block_hours, частота — delist.refresh_s (настройки
    площадки ниже на том же экране).
  Кто вызывает: pages/Venues.svelte.
  От чего зависит: lib/delist.svelte.js, lib/labels.js.
  Настройки: delist.refresh_s, delist.block_hours (только показ имён).
-->
<script>
  import { delist, delistOf, delistTime } from '../lib/delist.svelte.js';
  import { showTime } from '../lib/labels.js';

  /** venue — id площадки. */
  let { venue } = $props();
  const row = $derived(delistOf(venue));
</script>

{#if !row}
  <p class="muted">Делистинги не запрашивались: у DEX их нет, у биржи — ещё нет ответа (delist.refresh_s, venue.enabled).</p>
{:else}
  {#if row.got?.unknown}
    <p class="muted">Неизвестно: {row.got.unknown}. Сделки по делистингу с этой площадкой не блокируются.</p>
  {:else if row.got}
    {#if row.got.items.length}
      <table>
        <thead><tr><th>Монета</th><th>Пара</th><th>Торги до</th><th>Вывод до</th><th>Источник</th></tr></thead>
        <tbody>
          {#each row.got.items as i (i.symbol)}
            <tr>
              <td><strong>{i.ticker}</strong></td>
              <td><code>{i.symbol}</code></td>
              <td>{i.trading_end ? delistTime(i.trading_end) : 'не сообщено'}</td>
              <td>{i.withdraw_end ? delistTime(i.withdraw_end) : '—'}</td>
              <td class="muted">{i.source}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="ok">Пар в делистинге нет (по ответу биржи).</p>
    {/if}
  {:else}
    <p class="muted">Ответа биржи ещё не было.</p>
  {/if}
  {#if row.at}<p class="muted">Получено: {showTime(row.at)}</p>{/if}
  {#if row.error}<p class="err">Последний запрос не удался: {row.error}</p>{/if}
{/if}
{#if delist.error}<p class="err">{delist.error}</p>{/if}
