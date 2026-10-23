<!--
  Файл: ui/src/pages/Notifications.svelte
  Что делает: экран «Уведомления» — подключение Telegram (адрес Bot API, ID чата, таймаут HTTP,
    токен бота — переход к секрету на экране «Разблокировка», «Проверить подключение» → POST /api/check/telegram),
    интервал сводки и события из схемы (schema.events) по группам: по каждому вкл/выкл
    (notify.event.enabled), площадки (notify.event.venues, пусто — все), условие (notify.event.condition).
  Кто вызывает: App.svelte.
  От чего зависит: lib/settings.svelte.js, lib/secrets.svelte.js, lib/router.svelte.js,
    components/SettingEditor.svelte, CheckConnection.svelte.
  Настройки: notify.*, http.timeout_ms; секрет telegram.bot_token.
-->
<script>
  import { store, defOf } from '../lib/settings.svelte.js';
  import { secrets, TELEGRAM_TOKEN as TOKEN } from '../lib/secrets.svelte.js';
  import { href } from '../lib/router.svelte.js';
  import SettingEditor from '../components/SettingEditor.svelte';
  import CheckConnection from '../components/CheckConnection.svelte';

  // Имена настроек и секрета — из каталога бэкенда (src/settings/catalog.rs, src/secrets/mod.rs).
  const CONNECTION = ['notify.telegram_api_url', 'notify.telegram_chat', 'http.timeout_ms'];
  const SUMMARY = 'notify.summary_interval_s';
  const PER_EVENT = { enabled: 'notify.event.enabled', venues: 'notify.event.venues', condition: 'notify.event.condition' };

  const groups = $derived.by(() => {
    const out = new Map();
    for (const e of store.schema?.events ?? []) {
      if (!out.has(e.group)) out.set(e.group, []);
      out.get(e.group).push(e);
    }
    return [...out];
  });
  const tokenKnown = $derived(secrets.known.some((k) => k.name === TOKEN));
  const tokenEntered = $derived(secrets.status?.unlocked ? secrets.status.names.includes(TOKEN) : null);
</script>

{#snippet setting(name, scope, label, options)}
  {@const def = defOf(name)}
  {#if def}
    <SettingEditor {def} {scope} {label} {options} />
  {:else}
    <p class="err">Настройки {name} нет в схеме бэкенда.</p>
  {/if}
{/snippet}

<h1>Уведомления</h1>
{#if store.schema}
  <section class="card">
    <h2>Telegram</h2>
    {#each CONNECTION as name (name)}
      <p class="muted">{defOf(name)?.description}</p>
      {@render setting(name, { kind: 'Global' }, name)}
    {/each}
    <p class="row">
      Токен бота:
      {#if !tokenKnown}<span class="err">секрета {TOKEN} нет в списке бэкенда</span>
      {:else if tokenEntered === true}<span class="ok">введён</span>
      {:else if tokenEntered === false}<span class="unset">не введён</span>
      {:else}<span class="muted">введён ли — видно после разблокировки</span>{/if}
      <a href={href('unlock', TOKEN)}>Ввести или заменить на экране «Разблокировка»</a>
    </p>
    <CheckConnection url="/api/check/telegram" />
  </section>

  <section class="card">
    <h2>Сводка</h2>
    <p class="muted">{defOf(SUMMARY)?.description}</p>
    {@render setting(SUMMARY, { kind: 'Global' }, SUMMARY)}
  </section>

  <h2>События</h2>
  <p class="muted">
    Площадки: ничего не отмечено — для всех. Условие проверяется с этапа 5 (формат условия утверждается там),
    до этого не влияет на отправку.
  </p>
  {#each groups as [group, events] (group)}
    <h3>{group}</h3>
    {#each events as ev (ev.code)}
      <article class="card">
        <header><strong>{ev.title}</strong> <code>{ev.code}</code></header>
        {@render setting(PER_EVENT.enabled, { kind: 'Event', id: ev.code }, 'присылать', null)}
        {@render setting(PER_EVENT.venues, { kind: 'Event', id: ev.code }, 'площадки', store.venues)}
        {@render setting(PER_EVENT.condition, { kind: 'Event', id: ev.code }, 'условие (с этапа 5)', null)}
      </article>
    {/each}
  {/each}
{:else}
  <p class="muted">Загружаю схему…</p>
{/if}
