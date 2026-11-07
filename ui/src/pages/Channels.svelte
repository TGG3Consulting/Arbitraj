<!--
  Файл: ui/src/pages/Channels.svelte
  Что делает: экран «Каналы отправки» — каналы каждой площадки DEX (GET /api/channels/{venue}, через lib/dex.svelte.js):
    вкл/выкл, способ одиночная/бандл, минимальная цена газа, чаевые вкл/выкл + сумма, адрес, ключ,
    «Проверить подключение», состояние и статистика (отправлено, в блоке, откатов, газ впустую).
    Общие настройки отправки (send.*) — на «Площадках». Нет адаптера DEX — «нет данных» с причиной.
    Живое сообщение send_channel — список перечитывается (lib/dex.svelte.js). Над каналами — очередь кошелька
    (components/WalletQueue.svelte) — по каждому кошельку DEX (решение 132): состояние и «Снять очередь после ручной
    проверки».
  Кто вызывает: App.svelte.
  От чего зависит: lib/dex.svelte.js, components/ChannelCard.svelte, WalletQueue.svelte, NoData.svelte.
  Настройки: channel.* (через ChannelCard).
-->
<script>
  import { dex, whyNoDex } from '../lib/dex.svelte.js';
  import ChannelCard from '../components/ChannelCard.svelte';
  import WalletQueue from '../components/WalletQueue.svelte';
  import NoData from '../components/NoData.svelte';
  import { get } from '../lib/api.js';

  /** Кошельки DEX (решение 132): у каждого своя очередь. */
  let wallets = $state([]);
  $effect(() => {
    get('/api/wallets')
      .then((v) => (wallets = v.wallets))
      .catch(() => (wallets = []));
  });

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
    {#each uniqBy(wallets, (w) => w.id) as w (w.id)}
      <WalletQueue {venue} wallet={w.id} name={w.name} />
    {:else}
      <p class="muted">Кошельков DEX нет — очереди появятся на экране «Маппинг кошельков» (#/wallets).</p>
    {/each}
    {#each uniqBy(channels, (c) => c.info.id) as channel (channel.info.id)}
      <ChannelCard {venue} {channel} />
    {:else}
      <p class="muted">Адаптер сообщил пустой список каналов.</p>
    {/each}
  {:else}
    <NoData text={`Нет данных: каналы отправки даёт адаптер DEX (send_channels), сейчас ни одна площадка в шлюзе их не дала.${whyNoDex() ? ` ${whyNoDex()}` : ''}`} />
  {/each}
{/if}
