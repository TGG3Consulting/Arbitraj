<!--
  Файл: ui/src/pages/Channels.svelte
  Что делает: экран «Каналы отправки» — каналы каждой площадки DEX (GET /api/channels/{venue}, через lib/dex.svelte.js):
    вкл/выкл, способ одиночная/бандл, минимальная цена газа, чаевые вкл/выкл + сумма, адрес, ключ,
    «Проверить подключение», состояние и статистика (отправлено, в блоке, откатов, газ впустую).
    Общие настройки отправки (send.*) — на «Площадках». Нет адаптера DEX — «нет данных» с причиной.
    Живое сообщение send_channel — список перечитывается (lib/dex.svelte.js).
  Кто вызывает: App.svelte.
  От чего зависит: lib/dex.svelte.js, components/ChannelCard.svelte, NoData.svelte.
  Настройки: channel.* (через ChannelCard).
-->
<script>
  import { dex, whyNoDex } from '../lib/dex.svelte.js';
  import ChannelCard from '../components/ChannelCard.svelte';
  import NoData from '../components/NoData.svelte';

  /** Площадки, давшие список каналов: [[id, channels]]. */
  const withChannels = $derived(
    Object.entries(dex.byVenue)
      .filter(([, v]) => v.channels)
      .map(([id, v]) => [id, v.channels]),
  );
</script>

<h1>Каналы отправки</h1>
<p class="muted">
  Несколько закрытых каналов — отправка параллельно, без чаевых; открытый — только один (решение 33).
  Канал с ключом не используется, пока ключ не введён (решение 41).
</p>
{#if !dex.loaded}
  <p class="muted">Загружаю…</p>
{:else}
  {#each withChannels as [venue, channels] (venue)}
    <h2>{venue}</h2>
    {#each channels as channel (channel.info.id)}
      <ChannelCard {venue} {channel} />
    {:else}
      <p class="muted">Адаптер сообщил пустой список каналов.</p>
    {/each}
  {:else}
    <NoData text={`Нет данных: каналы отправки даёт адаптер DEX (send_channels), сейчас ни одна площадка в шлюзе их не дала.${whyNoDex() ? ` ${whyNoDex()}` : ''}`} />
  {/each}
{/if}
