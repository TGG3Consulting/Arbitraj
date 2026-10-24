<!--
  Файл: ui/src/components/ControlBar.svelte
  Что делает: плашка режима системы на всех экранах (решение 46): «Стоп» (после запуска — до команды «Старт»),
    «В работе», «Пауза» и причина; кнопки «Старт», «Пауза», «Стоп»; паузы площадок и карантины монет
    с кнопками снятия; итог дня — всего, факт и предварительная часть, неразобранные предварительные убытки сделок (решение 53). Команды уходят ядру (POST /api/control…) и пишутся в журнал.
  Кто вызывает: App.svelte.
  От чего зависит: lib/control.svelte.js, lib/coreLabels.js.
  Настройки: нет.
-->
<script>
  import { control, setMode, resumeVenue, releaseCoin } from '../lib/control.svelte.js';
  import { modeLabel } from '../lib/coreLabels.js';

  const r = $derived(control.data);
  const cls = $derived(r?.mode === 'Running' ? 'run' : r?.mode === 'Paused' ? 'warn' : 'stop');
</script>

<div class="banner control {cls}" role="status">
  {#if r}
    <strong>Режим: {modeLabel(r.mode)}</strong>
    {#if r.reason}<span>— {r.reason}</span>{/if}
    <span class="muted" title="Предварительный — убыток по худшему варианту остановленных сделок и «Беру на себя» (решение 53); после разбора заменяется фактическим">
      · итог дня {r.day.date}: {r.day.result_usd} USDT (факт {r.day.final_usd}, предварительно {r.day.provisional_usd})
    </span>
    <span class="row inline">
      <button disabled={control.busy || r.mode === 'Running'} onclick={() => setMode('Running')}>Старт</button>
      <button class="quiet" disabled={control.busy || r.mode !== 'Running'} onclick={() => setMode('Paused')}>Пауза</button>
      <button class="quiet" disabled={control.busy || r.mode === 'Stopped'} onclick={() => setMode('Stopped')}>Стоп</button>
    </span>
    {#each Object.entries(r.venue_paused) as [v, why] (v)}
      <div>Площадка <code>{v}</code> на паузе: {why} <button class="quiet" onclick={() => resumeVenue(v)}>Снять паузу</button></div>
    {/each}
    {#each Object.entries(r.day.provisional) as [deal, [usd, day]] (deal)}
      <div class="muted">Предварительный убыток сделки <code>{deal}</code>: {usd} USDT (учтён {day}) — ждёт разбора</div>
    {/each}
    {#each Object.entries(r.quarantine) as [t, why] (t)}
      <div>Монета <code>{t}</code> в карантине: {why} <button class="quiet" onclick={() => releaseCoin(t)}>Снять карантин</button></div>
    {/each}
  {:else}
    <span class="muted">Режим системы неизвестен{control.error ? `: ${control.error}` : ' — загружаю…'}</span>
  {/if}
  {#if r && control.error}<span class="err">{control.error}</span>{/if}
</div>
