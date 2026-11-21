<!--
  Файл: ui/src/components/TickerConflict.svelte
  Что делает: решение 211 на экране «Монеты» — только показ конфликтов «один адрес — два тикера на ОДНОЙ бирже»
    (GET /api/coins/ticker-splits): у адреса контракта на одной бирже несколько тикеров (старый/новый), и ядро не
    смогло выбрать торгуемый — монета на этой бирже не торгуется. Кнопок нет: монета = адрес, склейки тикеров нет
    (решения 200/206 убраны). Перечитывается при открытии экрана и по кнопке «Обновить».
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/api.js, components/ErrorText.svelte.
  Настройки: нет.
-->
<script>
  import { get } from '../lib/api.js';
  import ErrorText from './ErrorText.svelte';

  /** [{address, venues: [[биржа, тикер], …]}] — как отдаёт бэкенд (core/ticker_index.rs TickerSplit). */
  let list = $state([]);
  let error = $state('');

  /** Перечитывает конфликты. Ошибка — в error. */
  async function load() {
    try {
      list = await get('/api/coins/ticker-splits');
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  load();
</script>

<section class="card">
  <h2>Один адрес — два тикера на одной бирже</h2>
  <p class="muted">
    Монета = адрес контракта. Здесь — адреса, у которых на одной бирже несколько тикеров и выбрать торгуемый нельзя:
    на этой бирже монета не торгуется.
  </p>
  {#each list as s (s.address)}
    <p>
      <code>{s.address}</code> —
      {#each s.venues as [v, t], i (`${v}|${t}`)}{i ? ', ' : ''}{v}: <strong>{t}</strong>{/each}
    </p>
  {:else}
    <p class="muted">Конфликтов нет.</p>
  {/each}
  <button type="button" class="quiet" onclick={load}>Обновить</button>
  <ErrorText text={error} />
</section>
