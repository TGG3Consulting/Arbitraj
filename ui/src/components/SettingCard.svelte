<!--
  Файл: ui/src/components/SettingCard.svelte
  Что делает: карточка одной настройки на экране «Настройки»: имя, описание, единица, вид значения,
    ограничение, допустимые области и значения по областям:
    вся система; каждая площадка из GET /api/venues (у настроек времени в пути — подпись направлением
    перевода «bingx → кошелёк» / «кошелёк → биржа», решение 51); монеты — уже заданные + ввод адреса контракта или тикера с единственным адресом (решение 211, ревью М2:
    область «монета» — адрес 0x… в нижнем регистре; подпись — имя монеты из lib/coinNames.svelte.js; старые области по тикеру показываются как есть);
    события — уже заданные + выбор кода из схемы; DEX / вид пула и каналы — id от адаптера DEX
    (lib/dex.svelte.js) + уже заданные (такой id адаптер сейчас не сообщает — помечено). Решение 128: адаптер даёт
    DEX с видом, DEX из справочника пулов и id с уже заданными настройками; DEX, которой нет нигде (её пулы находит
    только поиск по фабрике, оракул или события Initialize), вводится руками — «Добавить DEX» в карточке настройки
    области «DEX» (правило id — checkDexId: строчная латиница, цифры, «-», «_», длина — dex.id_max_len). Строка
    появляется в этой карточке; после первого сохранения адаптер сообщает id сам — он во всех карточках области «DEX».
    Свои id каналов не вводятся: их даёт только адаптер. Кнопка «История».
    Решение 242: значения области «площадка + тип направления» задаются в подпункте «… › по биржам и направлениям»
    (components/VenueDirMoney.svelte) — здесь ссылка и сколько таких значений задано.
    Решение 248: у venue.cex_cex под значением каждой биржи — список «не взаимодействовать» и кнопка «Настроить список»
    (components/CexCexDeny.svelte); у самой venue.cex_cex_deny — только ссылка на это окно и «История».
  Кто вызывает: pages/Settings.svelte.
  От чего зависит: lib/settings.svelte.js, lib/dex.svelte.js, lib/labels.js, lib/coinNames.svelte.js, components/SettingEditor.svelte, SettingHistory.svelte, CexCexDeny.svelte.
  Настройки: любая — по описанию из схемы; dex.id_max_len (через checkDexId).
-->
<script>
  import { uniq, uniqBy } from '../lib/uniq.js';
  import { store, venueScopeLabel } from '../lib/settings.svelte.js';
  import { dex, dexIdsFor, allChannels, whyNoDex, checkDexId } from '../lib/dex.svelte.js';
  import { kindLabel, limitLabel, scopeLabel } from '../lib/labels.js';
  import { href } from '../lib/router.svelte.js';
  import { venueDirRoute } from '../lib/venueDir.js';
  import { names, coinName, ensure, coinIdOf } from '../lib/coinNames.svelte.js';
  import SettingEditor from './SettingEditor.svelte';
  import SettingHistory from './SettingHistory.svelte';
  import CexCexDeny from './CexCexDeny.svelte';

  // Справочник имён читается здесь напрямую (names.map) — загрузить его сразу (ревью М1).
  ensure();

  /** def — описание настройки из схемы. */
  let { def } = $props();
  let showHistory = $state(false);
  let added = $state({ Coin: [], Event: [], Dex: [] });
  let coinInput = $state('');
  /** Почему введённую монету нельзя добавить (ревью М2: тикер без единственного адреса); '' — можно. */
  let coinError = $state('');
  let eventInput = $state('');
  let dexInput = $state('');
  /** Причина, по которой введённый id DEX не подходит (решение 128); '' — подходит или поле пустое. */
  const dexInputError = $derived(dexInput.trim() ? checkDexId(dexInput.trim()) : '');

  /** Идентификаторы областей вида kind, для которых значение уже задано, плюс добавленные вручную. */
  function idsOf(kind) {
    const prefix = `${def.name}|${kind}|`;
    const set = new Set(added[kind]);
    for (const key of Object.keys(store.values)) if (key.startsWith(prefix)) set.add(key.slice(prefix.length));
    return [...set].sort();
  }

  /** Добавляет область для ввода значения (значение появится только после «Сохранить»). */
  function add(kind, id) {
    const v = id.trim();
    if (v && !added[kind].includes(v)) added[kind] = [...added[kind], v];
  }

  /**
   * Добавляет монету (решение 211, ревью М2): адрес — в нижнем регистре, как ключ бэкенда (settings/scope.rs coin_id);
   * тикер — его единственный адрес по справочнику имён, иначе подсказка ввести адрес. Возвращает: true — добавлена.
   */
  function addCoin(v) {
    try {
      add('Coin', coinIdOf(v));
      coinError = '';
      return true;
    } catch (e) {
      coinError = e.message;
      return false;
    }
  }

  /**
   * Области вида kind (Dex/Channel) для показа: от адаптера + уже заданные.
   * Принимает: вид. Возвращает: [{id, title, unknown}] — unknown: адаптер такой id сейчас не сообщает.
   */
  function adapterScopes(kind) {
    const fromAdapter =
      kind === 'Dex'
        ? dexIdsFor(def)
        : allChannels().map((c) => ({ id: c.info.id, title: c.info.title }));
    const out = new Map(fromAdapter.map((x) => [x.id, { ...x, unknown: false }]));
    for (const id of idsOf(kind)) if (!out.has(id)) out.set(id, { id, title: id, unknown: true });
    return [...out.values()];
  }

  /**
   * Подпись строки области DEX/канал. Принимает: вид, {id, title, unknown}. Возвращает: текст.
   * Решение 128: id, добавленный руками и ещё без значений, подписан «добавлена руками — задайте dex.kind».
   */
  function scopeTitle(kind, sc) {
    if (!sc.unknown) return `${sc.title} · ${sc.id}`;
    if (kind === 'Dex' && added.Dex.includes(sc.id)) return `${sc.id} (добавлена руками — задайте dex.kind и адреса)`;
    return `${sc.id} (адаптер не сообщает)`;
  }

  const eventTitle = (code) => store.schema.events.find((e) => e.code === code)?.title ?? code;
</script>

<article class="card">
  <header>
    <code>{def.name}</code>
    <span class="muted">
      {kindLabel(def.kind)}{def.unit ? `, ${def.unit}` : ''}{limitLabel(def.limit) ? `, ${limitLabel(def.limit)}` : ''}
      · области: {def.scopes.map(scopeLabel).join(', ')}
    </span>
  </header>
  <p>{def.description}</p>

  {#if def.scopes.includes('Global')}
    <SettingEditor {def} scope={{ kind: 'Global' }} label="вся система" />
  {/if}
  {#if def.name === 'venue.cex_cex_deny'}
    <!-- Решение 248: список задаётся окном «Настроить список» у venue.cex_cex (выбор бирж, не текст). -->
    <p class="muted">Задаётся кнопкой «Настроить список» в карточке venue.cex_cex (у каждой биржи).</p>
  {:else if def.scopes.includes('Venue')}
    {#each uniq(store.venues) as v (v)}
      <SettingEditor {def} scope={{ kind: 'Venue', id: v }} label={venueScopeLabel(def, v)} />
      {#if def.name === 'venue.cex_cex' && store.venueKinds.find((x) => x.id === v)?.kind === 'Cex'}
        <CexCexDeny venue={v} />
      {/if}
    {:else}
      <p class="muted">Площадок в шлюзе нет — значение по площадке задать не для чего.</p>
    {/each}
  {/if}
  {#if def.scopes.includes('Coin')}
    {#each uniq(idsOf('Coin')) as c (c)}
      <SettingEditor {def} scope={{ kind: 'Coin', id: c }} label={names.map[c] ? `монета ${coinName({ ticker: names.map[c].ticker, bsc_contract: c })} (${c})` : `монета ${c}`} />
    {/each}
    <div class="adder">
      <input bind:value={coinInput} placeholder="адрес монеты 0x… или тикер" />
      <button disabled={!coinInput.trim()} onclick={() => { if (addCoin(coinInput)) coinInput = ''; }}>
        Задать для монеты
      </button>
    </div>
    {#if coinError}<p class="err">{coinError}</p>{/if}
  {/if}
  {#if def.scopes.includes('Event')}
    {#each uniq(idsOf('Event')) as e (e)}
      <SettingEditor {def} scope={{ kind: 'Event', id: e }} label={eventTitle(e)} />
    {/each}
    <div class="adder">
      <select bind:value={eventInput}>
        <option value="">— событие —</option>
        {#each uniqBy(store.schema.events, (ev) => ev.code) as ev (ev.code)}<option value={ev.code}>{ev.group}: {ev.title}</option>{/each}
      </select>
      <button disabled={!eventInput} onclick={() => { add('Event', eventInput); eventInput = ''; }}>
        Задать для события
      </button>
    </div>
  {/if}

  {#each ['Dex', 'Channel'].filter((k) => def.scopes.includes(k)) as kind (kind)}
    {#each uniqBy(adapterScopes(kind), (sc) => sc.id) as sc (sc.id)}
      <SettingEditor {def} scope={{ kind, id: sc.id }} label={scopeTitle(kind, sc)} />
    {:else}
      <p class="nodata">
        Нет данных: {scopeLabel(kind)} — список даёт адаптер DEX, сейчас ни одна площадка его не дала.
        {dex.loaded && whyNoDex() ? whyNoDex() : ''}
      </p>
    {/each}
  {/each}
  {#if def.scopes.includes('Dex')}
    <!-- Решение 128: DEX, которой нет в списке адаптера (нет в справочнике пулов) — id вводится руками. -->
    <div class="adder">
      <input bind:value={dexInput} placeholder="id новой DEX, например lista-stable" />
      <button
        disabled={!dexInput.trim() || !!dexInputError}
        onclick={() => { add('Dex', dexInput); dexInput = ''; }}
      >
        Добавить DEX
      </button>
      {#if dexInputError}<span class="err">{dexInputError}</span>{/if}
    </div>
  {/if}

  {#if def.scopes.includes('VenueDir')}
    <p class="muted">
      По площадке и типу направления: задано {idsOf('VenueDir').length} —
      <a href={href('settings', venueDirRoute(store.schema))}>подпункт «по биржам и направлениям»</a>.
    </p>
  {/if}

  <button class="quiet" onclick={() => (showHistory = !showHistory)}>
    {showHistory ? 'Скрыть историю' : 'История'}
  </button>
  {#if showHistory}<SettingHistory name={def.name} />{/if}
</article>
