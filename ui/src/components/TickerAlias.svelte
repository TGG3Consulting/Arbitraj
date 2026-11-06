<!--
  Файл: ui/src/components/TickerAlias.svelte
  Что делает: решение 200 на экране «Монеты» — склейка тикеров одного адреса контракта. Список расхождений
    (GET /api/coins/ticker-splits: один адрес, у бирж разные тикеры — решение 118 делит монету, направлений нет):
    у каждого — кнопки «Считать одной монетой — основной X» (по каждому тикеру), подтверждение «Да, …» с адресом
    и тикерами → элемент «адрес=ОСНОВНОЙ,ДРУГИЕ» в coin.ticker_alias (PUT /api/settings, история настроек).
    Текущие склейки — с кнопкой «Убрать». После изменения шлюз сразу перечитывает справочник сетей бирж.
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/api.js, lib/settings.svelte.js, components/ErrorText.svelte.
  Настройки: coin.ticker_alias (src/settings/catalog/trade.rs).
-->
<script>
  import { get } from '../lib/api.js';
  import { store, setValue, valueKey } from '../lib/settings.svelte.js';
  import ErrorText from './ErrorText.svelte';

  const NAME = 'coin.ticker_alias';
  const SCOPE = { kind: 'Global' };

  let splits = $state([]);
  let error = $state('');
  /** Ждущее подтверждения: {address, main, others}; null — нет. */
  let ask = $state(null);

  const current = $derived(store.values[valueKey(NAME, SCOPE)] ?? []);

  /** Перечитывает расхождения. Ошибка — в error. */
  async function load() {
    try {
      splits = await get('/api/coins/ticker-splits');
      error = '';
    } catch (e) {
      error = e.message;
    }
  }
  $effect(() => {
    load();
  });

  /** Разные тикеры расхождения без повторов. */
  const tickers = (s) => [...new Set(s.venues.map((v) => v[1]))];

  /** Записывает склейку после «Да». Ошибка — в error. */
  async function confirm() {
    const item = `${ask.address}=${[ask.main, ...ask.others].join(',')}`;
    try {
      await setValue(NAME, SCOPE, [...current, item]);
      ask = null;
      await load();
    } catch (e) {
      error = e.message;
    }
  }

  /** Убирает элемент склейки. Ошибка — в error. */
  async function remove(item) {
    try {
      const rest = current.filter((x) => x !== item);
      await setValue(NAME, SCOPE, rest.length ? rest : null);
      await load();
    } catch (e) {
      error = e.message;
    }
  }
</script>

<h2>Один адрес — разные тикеры</h2>
<ErrorText text={error} />
{#if splits.length === 0}
  <p class="muted">Расхождений тикеров нет.</p>
{/if}
{#each splits as s (s.address)}
  <p>
    <code>{s.address}</code>: {s.venues.map((v) => `${v[0]}=${v[1]}`).join(', ')}
    {#each tickers(s) as t (t)}
      <button
        type="button"
        onclick={() => (ask = { address: s.address, main: t, others: tickers(s).filter((x) => x !== t) })}
        >Считать одной монетой — основной {t}</button
      >
    {/each}
  </p>
{/each}
{#if ask}
  <p>
    Адрес <code>{ask.address}</code>: тикеры {ask.others.join(', ')} считать монетой {ask.main}? Бот будет торговать
    между этими биржами; каждая биржа получает в запросах свой тикер.
    <button type="button" onclick={confirm}>Да, считать одной монетой {ask.main}</button>
    <button type="button" class="quiet" onclick={() => (ask = null)}>Отмена</button>
  </p>
{/if}
{#if current.length}
  <p class="muted">Склеено ({NAME}):</p>
  {#each current as item (item)}
    <p><code>{item}</code> <button type="button" class="quiet" onclick={() => remove(item)}>Убрать</button></p>
  {/each}
{/if}
