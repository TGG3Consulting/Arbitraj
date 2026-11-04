<!--
  Файл: ui/src/components/UncappedLine.svelte
  Что делает: одна строка расчёта «без потолка» (решение 141) — «без потолка: вложить X → прибыль Y; плюс от Z USDT;
    на нашем бюджете B — P; упущено M; ограничило: …». Для строки кандидата (поле uncapped) и карточки сделки.
  Кто вызывает: pages/Live.svelte, components/DealUncapped.svelte.
  От чего зависит: lib/uncappedLabels.js.
  Настройки: нет (uncapped.* — экран «Настройки», категория «Упущено из-за потолка»).
-->
<script>
  import { limitedLabel, money } from '../lib/uncappedLabels.js';

  /** u — {opt, opt_passes, min_plus, at_budget, budget, missed, limited_by, limit, at?}. */
  let { u } = $props();
</script>

<p class="uncapped">
  Без потолка: вложить <strong>{money(u.opt.invested)} USDT</strong> → прибыль <strong>{money(u.opt.profit)} USDT</strong>
  ({u.opt.qty} монет){#if !u.opt_passes} <span class="warn-text">— ниже порогов прибыли</span>{/if};
  {#if u.min_plus}плюс от <strong>{money(u.min_plus.invested)} USDT</strong>{:else}<span class="muted">не становится плюсовой в пределах ликвидности</span>{/if};
  на нашем бюджете {money(u.budget)} — {money(u.at_budget?.at?.profit)} USDT;
  упущено <strong class={Number(u.missed) > 0 ? 'err' : ''}>{money(u.missed)} USDT</strong>;
  ограничило: {limitedLabel(u.limited_by)}
  <span class="muted">· край: {u.limit}</span>
</p>

<style>
  .uncapped { margin: 4px 0; }
</style>
