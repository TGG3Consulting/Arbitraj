<!--
  Файл: ui/src/components/ChannelCard.svelte
  Что делает: один канал отправки на экране «Каналы отправки» (решения 33, 41–44): название, открытый/закрытый,
    способы по документации, ссылки; состояние и статистика (отправлено, отклонено, в блоке, откатов, газ впустую —
    SendChannelStats; null — «нет данных»); все настройки области «канал» (вкл/выкл, адрес, способ — только из
    способов канала, мин. цена газа, чаевые — только если у канала они есть); ключ канала; «Проверить подключение».
  Кто вызывает: pages/Channels.svelte.
  От чего зависит: lib/settings.svelte.js, lib/secrets.svelte.js, lib/labels.js, components/SettingEditor.svelte,
    SecretRow.svelte, CheckConnection.svelte.
  Настройки: все с областью Channel (channel.*, src/settings/catalog/send.rs); секрет из SendChannelInfo.secret.
-->
<script>
  import { store } from '../lib/settings.svelte.js';
  import { secrets } from '../lib/secrets.svelte.js';
  import { methodLabel, channelState, showTime } from '../lib/labels.js';
  import SettingEditor from './SettingEditor.svelte';
  import SecretRow from './SecretRow.svelte';
  import CheckConnection from './CheckConnection.svelte';

  /** venue — id площадки DEX; channel — {info: SendChannelInfo, stats: SendChannelStats | null}. */
  let { venue, channel } = $props();
  const info = $derived(channel.info);
  const stats = $derived(channel.stats);
  const state = $derived(stats ? channelState(stats.state) : null);

  // Настройки чаевых (channel.tip_*) показываются только у каналов с чаевыми по документации (SendChannelInfo.tips).
  const TIP_PREFIX = 'channel.tip_';
  const defs = $derived(
    (store.schema?.settings ?? []).filter(
      (d) => d.scopes.includes('Channel') && (info.tips || !d.name.startsWith(TIP_PREFIX)),
    ),
  );
  /** Способы канала в виде вариантов настройки: SendMethod «Single» ↔ вариант «single» (src/core/types/send.rs). */
  const methods = $derived(info.methods.map((m) => m.toLowerCase()));
  const secret = $derived(info.secret ? secrets.known.find((k) => k.name === info.secret) : null);
</script>

<article class="card">
  <header>
    <strong>{info.title}</strong> <code>{info.id}</code>
    <span class="tag">{info.private ? 'закрытый' : 'открытая отправка'}</span>
    <span class="muted">способы: {info.methods.map(methodLabel).join(', ') || '—'}; чаевые: {info.tips ? 'есть' : 'нет'}</span>
    {#each info.docs as url (url)}<a href={url} target="_blank" rel="noreferrer noopener">документация</a>{/each}
  </header>

  {#if stats}
    <p>Состояние: <span class={state.ok ? 'ok' : 'err'}>{state.text}</span> <span class="muted">· {showTime(stats.updated_at)}</span></p>
    <table>
      <thead><tr><th class="num">Отправлено</th><th class="num">Отклонено</th><th class="num">В блоке</th><th class="num">Откатов</th><th class="num">Газ впустую, BNB</th></tr></thead>
      <tbody><tr>
        <td class="num">{stats.sent}</td><td class="num">{stats.rejected}</td><td class="num">{stats.included}</td>
        <td class="num">{stats.reverted}</td><td class="num">{stats.gas_wasted_bnb}</td>
      </tr></tbody>
    </table>
  {:else}
    <p class="nodata">Нет данных: состояние и статистику канала ведёт ядро по событиям адаптера — пока не отдаёт.</p>
  {/if}

  {#each defs as def (def.name)}
    <div class="setting-line">
      <code>{def.name}</code> <span class="muted">{def.description}</span>
      <SettingEditor {def} scope={{ kind: 'Channel', id: info.id }} only={def.kind === 'Choice' && def.choices.some((c) => methods.includes(c)) ? methods : null} />
    </div>
  {/each}

  {#if info.secret}
    {#if secret}<SecretRow item={secret} />
    {:else}<p class="err">Секрета {info.secret} нет в списке бэкенда (src/secrets/mod.rs, KNOWN).</p>{/if}
  {/if}
  <CheckConnection url={`/api/check/channel/${encodeURIComponent(venue)}/${encodeURIComponent(info.id)}`} />
</article>
