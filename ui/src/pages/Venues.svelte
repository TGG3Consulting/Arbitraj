<!--
  Файл: ui/src/pages/Venues.svelte
  Что делает: экран «Площадки» — площадки из реестра шлюза (GET /api/venues); по каждой:
    «Проверить подключение» (POST /api/check/venue/{id}; у DEX — узел BSC и кошелёк), её секреты
    (GET /api/secrets/known, где scope = id; ключи каналов — в блоке каналов), её настройки области «площадка»
    по группам; у DEX — блоки «Справочник пулов (индексатор)», «DEX и виды пулов», «Каналы отправки» (lib/dex.svelte.js)
    и ссылка на «Маппинг кошельков» (исполнители ArbExecutor — у кошельков, решение 132).
    Секреты площадок, которых ещё нет в шлюзе (например адаптер DEX не зарегистрирован), тоже видны —
    ключ можно ввести заранее. Решение 173: итог дня площадки, её лимит дня, пауза по убытку и «Сбросить лимит дня»
    площадки (components/DayVenues.svelte). Решение 175: блок «Делистинги» биржи — пары в делистинге или «неизвестно»
    (components/Delistings.svelte, lib/delist.svelte.js). Статусы соединений, задержки и разрывы появятся, когда бэкенд начнёт их отдавать.
  Кто вызывает: App.svelte.
  От чего зависит: lib/settings.svelte.js, lib/secrets.svelte.js, lib/dex.svelte.js, components/CheckConnection.svelte,
    SecretRow.svelte, SettingEditor.svelte, PoolDirectory.svelte, DexFamilies.svelte, VenueChannels.svelte,
    ErrorText.svelte, DayVenues.svelte (lib/control.svelte.js), Delistings.svelte (lib/delist.svelte.js).
  Настройки: все с областью Venue; у DEX — с областями Dex (через DexFamilies).
-->
<script>
  import { uniq, uniqBy } from '../lib/uniq.js';
  import { store } from '../lib/settings.svelte.js';
  import { secrets, TELEGRAM_TOKEN } from '../lib/secrets.svelte.js';
  import { dex } from '../lib/dex.svelte.js';
  import CheckConnection from '../components/CheckConnection.svelte';
  import SecretRow from '../components/SecretRow.svelte';
  import SettingEditor from '../components/SettingEditor.svelte';
  import DexFamilies from '../components/DexFamilies.svelte';
  import VenueChannels from '../components/VenueChannels.svelte';
  import PoolDirectory from '../components/PoolDirectory.svelte';
  import ErrorText from '../components/ErrorText.svelte';
  import DayVenues from '../components/DayVenues.svelte';
  import Delistings from '../components/Delistings.svelte';
  import { startDelist } from '../lib/delist.svelte.js';

  startDelist();

  const groups = $derived.by(() => {
    const out = new Map();
    for (const d of store.schema?.settings ?? []) {
      if (!d.scopes.includes('Venue')) continue;
      if (!out.has(d.group)) out.set(d.group, []);
      out.get(d.group).push(d);
    }
    return [...out];
  });

  /** Имена секретов, которые показаны в блоке каналов площадки. */
  const channelSecrets = (id) => new Set((dex.byVenue[id]?.channels ?? []).map((c) => c.info.secret).filter(Boolean));
  /** Секреты площадки без секретов каналов. */
  const venueKeys = (id) => secrets.known.filter((k) => k.scope === id && !channelSecrets(id).has(k.name));

  /** Секреты площадок, которых нет в шлюзе (без секрета Telegram — он на экране «Уведомления»). */
  const telegramScope = $derived(secrets.known.find((k) => k.name === TELEGRAM_TOKEN)?.scope);
  const pending = $derived.by(() => {
    const out = new Map();
    for (const k of secrets.known) {
      if (k.scope === telegramScope || store.venues.includes(k.scope)) continue;
      if (!out.has(k.scope)) out.set(k.scope, []);
      out.get(k.scope).push(k);
    }
    return [...out];
  });
</script>

<h1>Площадки</h1>
<ErrorText text={store.error} />
<ErrorText text={secrets.error} />
{#each uniq(store.venues) as id (id)}
  {@const d = dex.byVenue[id]}
  <section class="card venue">
    <h2>{id}</h2>
    <CheckConnection url={`/api/check/venue/${encodeURIComponent(id)}`} />

    <h3>Итог дня и лимит дня</h3>
    <DayVenues only={id} />

    <h3>Делистинги (решение 175)</h3>
    <Delistings venue={id} />

    <h3>Ключи</h3>
    {#each uniqBy(venueKeys(id), (item) => item.name) as item (item.name)}
      <SecretRow {item} />
    {:else}
      <p class="muted">У площадки нет секретов в списке бэкенда.</p>
    {/each}

    {#if d?.directory}<PoolDirectory dir={d.directory} />{/if}
    <ErrorText text={d?.directoryError} />
    {#if d?.families}<DexFamilies families={d.families} dir={d.directory} />{/if}
    <ErrorText text={d?.familiesError} />
    {#if d?.channels}<VenueChannels venue={id} channels={d.channels} />{/if}
    <ErrorText text={d?.channelsError} />
    {#if d?.families}
      <p class="muted">Кошельки DEX, их исполнители ArbExecutor и маппинг «биржа → кошелёк» — экран <a href="#/wallets">«Маппинг кошельков»</a> (решение 132).</p>
    {/if}

    <h3>Настройки площадки</h3>
    {#each groups as [group, defs] (group)}
      <h4>{group}</h4>
      {#each defs as def (def.name)}
        <div class="setting-line">
          <code>{def.name}</code> <span class="muted">{def.description}{def.unit ? ` (${def.unit})` : ''}</span>
          <SettingEditor {def} scope={{ kind: 'Venue', id }} />
        </div>
      {/each}
    {/each}
  </section>
{:else}
  <p class="muted">{store.schema ? 'В шлюзе нет зарегистрированных площадок.' : 'Загружаю…'}</p>
{/each}

{#each pending as [scope, items] (scope)}
  <section class="card venue">
    <h2>{scope} <span class="tag">нет в шлюзе</span></h2>
    <p class="muted">
      Адаптер площадки не зарегистрирован в шлюзе: проверка подключения ответит причиной, списки DEX и каналов
      и настройки площадки появятся, когда адаптер подключится. Ключи можно ввести заранее.
    </p>
    <CheckConnection url={`/api/check/venue/${encodeURIComponent(scope)}`} />
    <h3>Ключи</h3>
    {#each uniqBy(items, (item) => item.name) as item (item.name)}<SecretRow {item} />{/each}
  </section>
{/each}
