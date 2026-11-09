<!--
  Файл: ui/src/components/VenueDirMoney.svelte
  Что делает: решение 242 — подпункт «Деньги и риск › по биржам и направлениям». Блок каждой биржи (GET /api/venues/kinds,
    вид Cex) с тремя рубриками: «DEX → биржа» (DEX → эта биржа), «биржа → DEX», «биржа → биржа» (покупка на этой бирже);
    отдельный блок «DEX ↔ DEX» — один на все циклы (Тигран 09.11: не по DEX и не по кошельку). В рубрике — строки
    настроек с областью VenueDir, у которых этот тип есть в dir_kinds схемы: поле, «Сохранить», «Очистить»
    (SettingEditor), «История» строки (история только этой области). Пустое поле — подпись, что действует
    (lib/venueDir.js inheritedText: площадка / монета / вся система).
  Кто вызывает: pages/Settings.svelte.
  От чего зависит: lib/settings.svelte.js, lib/venueDir.js, components/SettingEditor.svelte, SettingHistory.svelte.
  Настройки: money.min_profit_usd, money.min_profit_pct, money.max_per_trade_usd, profit.price_move_k,
    profit.closure_share (все с областью VenueDir).
-->
<script>
  import { uniq } from '../lib/uniq.js';
  import { store } from '../lib/settings.svelte.js';
  import {
    CEX_KINDS, DEX_DEX, venueDirDefs, venueDirId, venueDirScopeKey, inheritedText,
  } from '../lib/venueDir.js';
  import SettingEditor from './SettingEditor.svelte';
  import SettingHistory from './SettingHistory.svelte';

  const defs = $derived(venueDirDefs(store.schema));
  const cex = $derived(uniq(store.venueKinds.filter((x) => x.kind === 'Cex').map((x) => x.id)));
  /** Открытая история: ключ «имя|ключ области» или ''. */
  let open = $state('');

  /** Настройки рубрики — у которых этот тип направления есть в схеме. */
  const defsOf = (key) => defs.filter((d) => d.dir_kinds.includes(key));
</script>

{#snippet rubric(venue, rub)}
  <h4>{rub.title}</h4>
  {#each defsOf(rub.key) as def (def.name)}
    {@const sk = venueDirScopeKey(venue, rub.key)}
    <div class="row">
      <SettingEditor
        {def}
        scope={{ kind: 'VenueDir', id: venueDirId(venue, rub.key) }}
        label={`${def.name}${def.unit ? `, ${def.unit}` : ''}`}
        inherited={inheritedText(def, rub, venue, store.values, store.venueKinds)}
      />
      <button class="quiet" onclick={() => (open = open === `${def.name}|${sk}` ? '' : `${def.name}|${sk}`)}>
        {open === `${def.name}|${sk}` ? 'Скрыть историю' : 'История'}
      </button>
      {#if open === `${def.name}|${sk}`}<SettingHistory name={def.name} scope={sk} />{/if}
    </div>
  {/each}
{/snippet}

<section class="vd">
  <p class="muted">
    Значение пары «площадка · направление» сильнее общего. Не задано — действует общее (подпись у поля): площадка, монета
    или вся система, как раньше. У биржи → DEX и биржи → биржи — значение биржи покупки; у DEX → биржи — значение биржи
    продажи; DEX ↔ DEX — один набор на все циклы. Исключение (Тигран 09.11): у profit.price_move_k значение монеты сильнее
    пары — монета → пара → вся система.
  </p>
  {#each cex as v (v)}
    <article class="card">
      <h3>{v}</h3>
      {#each CEX_KINDS as rub (rub.key)}{@render rubric(v, rub)}{/each}
    </article>
  {:else}
    <p class="nodata">Нет данных: бирж в шлюзе нет — значения по бирже задать не для чего.</p>
  {/each}
  {#if defsOf(DEX_DEX.key).length}
    <article class="card">
      <h3>{DEX_DEX.title}</h3>
      {@render rubric('', DEX_DEX)}
    </article>
  {/if}
</section>

<style>
  h3 { margin: 0 0 4px; }
  h4 { margin: 10px 0 2px; font-size: 14px; }
  .row { margin-left: 8px; }
</style>
