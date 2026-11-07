<!--
  Файл: ui/src/components/TickerAlias.svelte
  Что делает: решение 200 на экране «Монеты» — склейка тикеров одного адреса контракта. Список расхождений
    (GET /api/coins/ticker-splits: один адрес, у бирж разные тикеры — решение 118 делит монету, направлений нет):
    у каждого — кнопки «Считать одной монетой — основной X» (по каждому тикеру), подтверждение «Да, …» с адресом
    и тикерами → элемент «адрес=ОСНОВНОЙ,ДРУГИЕ» в coin.ticker_alias (PUT /api/settings, история настроек).
    Текущие склейки — с кнопкой «Убрать». После изменения шлюз сразу перечитывает справочник сетей бирж.
    Решение 206: переключатель coin.ticker_alias_auto «склеивать автоматически все тикеры одного адреса»; список
    склеенного автоматически (GET /api/coins/ticker-aliases-auto) с кнопкой «Разделить» (адрес → coin.ticker_alias_off,
    подтверждение «Да, …»); разделённые — с кнопкой «Вернуть».
  Кто вызывает: pages/Coins.svelte.
  От чего зависит: lib/api.js, lib/settings.svelte.js, components/ErrorText.svelte.
  Настройки: coin.ticker_alias, coin.ticker_alias_auto, coin.ticker_alias_off (src/settings/catalog/trade.rs).
-->
<script>
  import { uniq, uniqBy } from '../lib/uniq.js';
  import { get } from '../lib/api.js';
  import { store, setValue, valueKey } from '../lib/settings.svelte.js';
  import ErrorText from './ErrorText.svelte';

  const NAME = 'coin.ticker_alias';
  const AUTO = 'coin.ticker_alias_auto';
  const OFF = 'coin.ticker_alias_off';
  const SCOPE = { kind: 'Global' };

  let splits = $state([]);
  let autos = $state([]);
  /** Адрес, ждущий подтверждения «Разделить»; null — нет. */
  let askOff = $state(null);
  let error = $state('');
  /** Ждущее подтверждения: {address, main, others}; null — нет. */
  let ask = $state(null);

  const current = $derived(store.values[valueKey(NAME, SCOPE)] ?? []);
  const auto = $derived(store.values[valueKey(AUTO, SCOPE)]);
  const off = $derived(store.values[valueKey(OFF, SCOPE)] ?? []);

  /** Перечитывает расхождения. Ошибка — в error. */
  async function load() {
    try {
      [splits, autos] = await Promise.all([get('/api/coins/ticker-splits'), get('/api/coins/ticker-aliases-auto')]);
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

  /** Решение 206: включает или выключает автоматическую склейку. Ошибка — в error. */
  async function setAuto(v) {
    try {
      await setValue(AUTO, SCOPE, v);
      await load();
    } catch (e) {
      error = e.message;
    }
  }

  /** Решение 206: «Разделить» (после «Да») или «Вернуть» адрес. Ошибка — в error. */
  async function setOff(address, on) {
    try {
      const rest = off.filter((x) => x.toLowerCase() !== address.toLowerCase());
      const next = on ? [...rest, address] : rest;
      await setValue(OFF, SCOPE, next.length ? next : null);
      askOff = null;
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
<p>
  Склеивать автоматически все тикеры одного адреса ({AUTO}):
  <strong>{auto === true ? 'да' : auto === false ? 'нет' : 'не задано'}</strong>
  <button type="button" onclick={() => setAuto(auto !== true)}>{auto === true ? 'Выключить' : 'Включить'}</button>
</p>
{#if autos.length}
  <p class="muted">Склеено автоматически ({autos.length}):</p>
  {#each uniqBy(autos, (a) => a.address) as a (a.address)}
    <p>
      <strong>{a.main}</strong> <code>{a.address}</code>: {a.venues.map((v) => `${v[0]}=${v[1]}`).join(', ')}
      <button type="button" class="quiet" onclick={() => (askOff = a)}>Разделить</button>
    </p>
  {/each}
{/if}
{#if askOff}
  <p>
    Адрес <code>{askOff.address}</code>: не склеивать автоматически (биржи снова увидят разные монеты, направлений
    между ними не будет)?
    <button type="button" onclick={() => setOff(askOff.address, true)}>Да, разделить</button>
    <button type="button" class="quiet" onclick={() => (askOff = null)}>Отмена</button>
  </p>
{/if}
{#if off.length}
  <p class="muted">Разделено ({OFF}):</p>
  {#each uniq(off) as x (x)}
    <p><code>{x}</code> <button type="button" class="quiet" onclick={() => setOff(x, false)}>Вернуть</button></p>
  {/each}
{/if}
{#if splits.length === 0}
  <p class="muted">Расхождений тикеров нет.</p>
{/if}
{#each uniqBy(splits, (s) => s.address) as s (s.address)}
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
  {#each uniq(current) as item (item)}
    <p><code>{item}</code> <button type="button" class="quiet" onclick={() => remove(item)}>Убрать</button></p>
  {/each}
{/if}
