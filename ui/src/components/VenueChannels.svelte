<!--
  Файл: ui/src/components/VenueChannels.svelte
  Что делает: блок «Каналы отправки» площадки DEX на экране «Площадки»: по каждому каналу от адаптера
    (GET /api/channels/{venue}) — название, открытый/закрытый, способы отправки, ссылки на документацию,
    поле ключа (секрет из SendChannelInfo.secret, если каналу нужен ключ; значение не показывается никогда)
    и «Проверить подключение» (POST /api/check/channel/{venue}/{id}). Настройки каналов — область «канал».
  Кто вызывает: pages/Venues.svelte.
  От чего зависит: lib/secrets.svelte.js, lib/labels.js, components/SecretRow.svelte,
    CheckConnection.svelte.
  Настройки: нет (channel.* — экран «Настройки», область «канал»); секреты channel.<id>.key.
-->
<script>
  import { secrets } from '../lib/secrets.svelte.js';
  import { methodLabel } from '../lib/labels.js';
  import SecretRow from './SecretRow.svelte';
  import CheckConnection from './CheckConnection.svelte';

  /** venue — id площадки; channels — [{info, stats}]. */
  let { venue, channels } = $props();

  /** Секрет канала из списка бэкенда. Принимает: имя. Возвращает: {name, scope, description} или undefined. */
  const known = (name) => secrets.known.find((k) => k.name === name);
</script>

<h3>Каналы отправки</h3>
<p class="muted">Включение, способ, газ и чаевые — область «канал» на экране «Настройки».</p>
{#each channels as { info } (info.id)}
  <div class="card">
    <header>
      <strong>{info.title}</strong> <code>{info.id}</code>
      <span class="tag">{info.private ? 'закрытый' : 'открытая отправка'}</span>
      <span class="muted">способы: {info.methods.map(methodLabel).join(', ') || '—'}</span>
      {#each info.docs as url (url)}<a href={url} target="_blank" rel="noreferrer noopener">документация</a>{/each}
    </header>
    {#if info.secret}
      {#if known(info.secret)}
        <SecretRow item={known(info.secret)} />
      {:else}
        <p class="err">Секрета {info.secret} нет в списке бэкенда (src/secrets/mod.rs, KNOWN).</p>
      {/if}
    {:else}
      <p class="muted">Ключ каналу не нужен.</p>
    {/if}
    <CheckConnection url={`/api/check/channel/${encodeURIComponent(venue)}/${encodeURIComponent(info.id)}`} />
  </div>
{:else}
  <p class="nodata">Нет данных: адаптер не сообщил ни одного канала.</p>
{/each}
