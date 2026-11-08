<!--
  Файл: ui/src/components/UncappedLine.svelte
  Что делает: одна строка расчёта «без потолка» (решение 141) — «без потолка: вложить X → прибыль Y; плюс от Z USDT;
    на нашем бюджете B — P; упущено M; ограничило: …». Для строки кандидата (поле uncapped) и карточки сделки.
    Решение 217: у записи сделки — на каких данных посчитано: «по рынку на момент решения» (снимок до ордера покупки) или
    «посчитано после покупки — занижено» (старые записи). Решение 218: запись, пересчитанная по окну сделки, — пометка
    «пересчитано позже по записанному окну» (источник: стаканы окна; у биржа → DEX — DEX из плана, приблизительно).
    Решение 236: у сделки — «потолок» (сколько можно было вложить с максимальной прибылью без лимита), «заработали бы /
    потеряли бы на потолке» (расчёт по данным момента входа), «на вложенном» и «упущено против вложенного»; запись до
    решения 236 (σ и курсы позже входа) — пометка «несравнимо».
  Кто вызывает: pages/Live.svelte, components/DealUncapped.svelte.
  От чего зависит: lib/uncappedLabels.js, lib/labels.js (showTime).
  Настройки: нет (uncapped.* — экран «Настройки», категория «Упущено из-за потолка»).
-->
<script>
  import { basisLabel, lateInputs, limitedLabel, money, recomputed, understated, wouldText } from '../lib/uncappedLabels.js';
  import { showTime } from '../lib/labels.js';

  /** u — {opt, opt_passes, min_plus, at_budget, budget, missed, limited_by, limit, at?}. */
  let { u } = $props();
  const deal = $derived(u?.battle?.verdict === 'deal');
</script>

{#if understated(u)}
  <p class="warn-text">Посчитано после нашей покупки — занижено (запись до решения 217: стакан уже съеден нашей покупкой).</p>
{:else if recomputed(u)}
  <p class="muted">Пересчитано позже по записанному окну сделки ({basisLabel(u.basis)}), рынок на момент решения о входе: {showTime(u.decided_at)}</p>
{:else if lateInputs(u)}
  <p class="warn-text">Стаканы — на момент входа ({showTime(u.decided_at)}), но σ и курсы взяты позже, при расчёте (запись до решения 236) — с решением о входе несравнимо.</p>
{:else if u.decided_at}
  <p class="muted">Расчёт по данным момента входа (до ордера покупки): {showTime(u.decided_at)}</p>
{/if}
<p class="uncapped">
  {#if deal}
    Потолок (сколько можно было вложить с максимальной прибылью без лимита): <strong>{money(u.opt.invested)} USDT</strong>
    ({u.opt.qty} монет) — на потолке {wouldText(u.opt.profit)} <strong>{money(Math.abs(Number(u.opt.profit)))} USDT</strong> (расчёт){#if !u.opt_passes} <span class="warn-text">— ниже порогов прибыли</span>{/if};
    на вложенных {money(u.at_budget?.at?.invested)} — {money(u.at_budget?.at?.profit)} USDT (тот же расчёт);
    упущено против вложенного <strong class={Number(u.missed) > 0 ? 'err' : ''}>{money(u.missed)} USDT</strong>;
  {:else}
    Без потолка: вложить <strong>{money(u.opt.invested)} USDT</strong> → прибыль <strong>{money(u.opt.profit)} USDT</strong>
    ({u.opt.qty} монет){#if !u.opt_passes} <span class="warn-text">— ниже порогов прибыли</span>{/if};
    на нашем бюджете {money(u.budget)} — {money(u.at_budget?.at?.profit)} USDT;
    упущено <strong class={Number(u.missed) > 0 ? 'err' : ''}>{money(u.missed)} USDT</strong>;
  {/if}
  {#if u.min_plus}плюс от <strong>{money(u.min_plus.invested)} USDT</strong>{:else}<span class="muted">не становится плюсовой в пределах ликвидности</span>{/if};
  ограничило: {limitedLabel(u.limited_by)}
  <span class="muted">· край: {u.limit}</span>
</p>

<style>
  .uncapped { margin: 4px 0; }
</style>
