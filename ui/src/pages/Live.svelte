<!--
  Файл: ui/src/pages/Live.svelte
  Что делает: «Живой экран» во всю ширину окна — компактная строка на монету (решение 55, вид «Б» — решение 106):
    лучшее направление монеты (спред, порог комиссий, состояние), сколько направлений и сколько выше порога, время;
    щелчок по строке раскрывает все направления, «подробно» — окно с адресом, ценами и полными причинами;
    сортировка по наибольшему спреду; и последние расчёты кандидатов:
    прошёл (прибыль, объём) или отказ с причиной; прошёл, а сделки нет — причина отказа исполнителя (поле start,
    решение 147); по кандидату раскрывается полный список затрат.
    Данные — GET /api/spreads, /api/candidates; живые сообщения spread и candidate обновляют строки на месте.
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/live.svelte.js, lib/coreLabels.js, lib/labels.js, components/ErrorText.svelte,
    components/LiveCoins.svelte, components/CostLines.svelte, components/DryBanner.svelte, components/SignalCard.svelte,
    components/DexCycles.svelte (сигнал циклов DEX ↔ DEX, решение 85), components/CalcTiming.svelte (время расчёта по
    этапам, решение 129).
  Настройки: нет (пороги signal.*, money.*, profit.*, подбор объёма size.* — экран «Настройки»).
  Кандидат показывает, как подобран объём (calc.size.why, решение 66): оптимум, прибыль, прибыль на бюджете;
    в режиме «лучший из двух» (решение 123) — кто дал финальную котировку DEX и на сколько USDT (Quote.pick).
  Решение 74: вверху — плашка режима (сухой / обычный / не задан, DryBanner); у прошедшего — метка «сухой» и
    раскрываемая карточка сигнала (SignalCard: стакан и пул в полосе, максимальный объём и прибыль, ссылки).
  Решение 175: у кандидата — отметка «делистинг на X: дата» по сторонам направления (lib/delist.svelte.js).
  Решение 141: у кандидата — строка «без потолка: вложить X → прибыль Y; плюс от Z» (UncappedLine, поле uncapped).
  Решение 198 (06.11.2026: таблица застывала, время «Обновлено» отставало на минуты, таймер страницы опаздывал на 8 с):
    живые сообщения spread / candidate копятся в буфере (последнее по направлению) и применяются одной пачкой в кадре
    браузера (requestAnimationFrame), не чаще live.spread_min_interval_ms (то же значение, с которым сервер шлёт
    строку направления; не задана — каждый кадр). Раньше каждое из тысяч сообщений в секунду пересчитывало всю
    таблицу монет (~3700 направлений) — страница не успевала, данные на экране старели при свежих на сервере.
-->
<script>
  import { get } from '../lib/api.js';
  import { onLive, lagFor } from '../lib/live.svelte.js';
  import { store, valueKey } from '../lib/settings.svelte.js';
  import { dirText, dirKey, pickLines } from '../lib/coreLabels.js';
  import { showTime } from '../lib/labels.js';
  import ErrorText from '../components/ErrorText.svelte';
  import CostLines from '../components/CostLines.svelte';
  import LiveCoins from '../components/LiveCoins.svelte';
  import DryBanner from '../components/DryBanner.svelte';
  import SignalCard from '../components/SignalCard.svelte';
  import DexCycles from '../components/DexCycles.svelte';
  import UncappedLine from '../components/UncappedLine.svelte';
  import CalcTiming from '../components/CalcTiming.svelte';
  import { startDelist, delistMarks } from '../lib/delist.svelte.js';

  startDelist();

  // $state.raw: объект заменяется целиком пачкой (решение 198), без глубокого отслеживания тысяч строк.
  let spreads = $state.raw({});
  let cands = $state.raw({});
  /** Буфер живых сообщений до следующей пачки: направление → последнее сообщение. */
  let pendS = new Map();
  let pendC = new Map();
  let frame = 0;
  let lastFlush = 0;
  let error = $state('');
  let filter = $state('');
  let onlyActive = $state(false);

  /** Перечитывает оба списка. Ошибка — в error. */
  async function reload() {
    try {
      const [s, c] = await Promise.all([get('/api/spreads'), get('/api/candidates')]);
      spreads = Object.fromEntries(s.map((v) => [dirKey(v.direction), v]));
      cands = Object.fromEntries(c.map((v) => [dirKey(v.direction), v]));
      error = '';
    } catch (e) {
      error = e.message;
    }
  }

  /** Наименьший промежуток между пачками, мс: live.spread_min_interval_ms (глобальная); не задана — 0. */
  const minGap = () => Number(store.values[valueKey('live.spread_min_interval_ms', { kind: 'Global' })] ?? 0);

  /** Применяет накопленные сообщения одной заменой объектов (решение 198). Ошибок нет. */
  function flush(now) {
    frame = 0;
    if (now - lastFlush < minGap()) {
      frame = requestAnimationFrame(flush);
      return;
    }
    lastFlush = now;
    if (pendS.size) {
      spreads = Object.assign({ ...spreads }, Object.fromEntries(pendS));
      pendS = new Map();
    }
    if (pendC.size) {
      cands = Object.assign({ ...cands }, Object.fromEntries(pendC));
      pendC = new Map();
    }
  }

  /** Ставит пачку в ближайший кадр, если ещё не стоит. Ошибок нет. */
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(flush);
  };

  $effect(() => {
    reload();
    const off = onLive((msg) => {
      if (msg.type === 'spread') {
        pendS.set(dirKey(msg.data.direction), msg.data);
        schedule();
      } else if (msg.type === 'candidate') {
        pendC.set(dirKey(msg.data.direction), msg.data);
        schedule();
      } else if (lagFor(msg, 'core') || msg.type === 'coins') reload();
    });
    return () => {
      off();
      if (frame) cancelAnimationFrame(frame);
    };
  });

  /** Фильтр по тикеру монеты (Asset). */
  const matchAsset = (a) => a.ticker.toLowerCase().includes(filter.trim().toLowerCase());
  const match = (d) => matchAsset(d.asset);
  const views = $derived(Object.values(spreads));
  const candRows = $derived(Object.values(cands).filter((c) => match(c.direction)).sort((a, b) => (b.at > a.at ? 1 : -1)));
</script>

<h1>Живой экран</h1>
<DryBanner always />
<ErrorText text={error} />
<div class="row">
  <input placeholder="монета" bind:value={filter} />
  <label class="inline"><input type="checkbox" bind:checked={onlyActive} /> только выше порога</label>
</div>

<h2>Спреды по монетам: лучшее направление (щелчок — все направления)</h2>
<LiveCoins {views} match={matchAsset} {onlyActive} />

<h2>Кандидаты: расчёт и причины отказа</h2>
{#each candRows as c (dirKey(c.direction))}
  {@const acc = c.verdict.Accept}
  {@const rej = c.verdict.Reject}
  {@const calc = acc ?? rej?.best}
  <article class="card">
    <header>
      <strong>{dirText(c.direction)}</strong>
      {#if c.dry}<span class="tag dry">СУХОЙ</span>{/if}
      {#if acc}<span class="ok">прошёл: прибыль {acc.profit_usd} USDT ({acc.profit_pct}%), объём {acc.qty}</span>
      {:else}<span class="err">отказ: {rej.reason}</span>{/if}
      <span class="muted">{showTime(c.at)}</span>
      {#each delistMarks(c.direction) as m (m)}<span class="err">{m}</span>{/each}
    </header>
    {#if c.start}
      <!-- Решение 147: расчёт прошёл, а исполнитель отказал на входе — точная причина. -->
      <p class="err">{c.start}</p>
    {/if}
    {#if c.timing}
      <CalcTiming timing={c.timing} />
    {/if}
    {#if c.uncapped}
      <UncappedLine u={c.uncapped} />
    {/if}
    {#if c.card}
      <details open>
        <summary>Карточка сигнала</summary>
        <SignalCard card={c.card} />
      </details>
    {/if}
    {#if calc?.size}
      <p class="muted">Объём (решение 66): {calc.size.why}</p>
    {/if}
    {#each pickLines(calc) as line, i (i)}
      <p>Исполнитель DEX, {line}</p>
    {/each}
    {#if calc}
      <details>
        <summary>Полный расчёт: вложено {calc.invested}, выручка {calc.proceeds}, затраты {calc.costs_usdt} USDT, T = {calc.transit_s} с, σ = {calc.sigma}</summary>
        <CostLines lines={calc.costs} />
      </details>
    {/if}
  </article>
{:else}
  <p class="muted">Кандидатов ещё не было.</p>
{/each}

<h2>Циклы DEX ↔ DEX: сигнал (решение 85)</h2>
<DexCycles />

<style>
  .tag.dry { background: #b45309; color: #fff; border-color: #b45309; font-weight: 700; }
</style>
