<!--
  Файл: ui/src/App.svelte
  Что делает: каркас интерфейса — меню экранов, плашки «Система заблокирована», «Нет живой связи»
    и режима системы («Старт/Стоп/Пауза»),
    выбор экрана по адресу (#/экран); «Живой экран» и «Монеты» — во всю ширину окна (Issue #33). Запускает живой поток и общие хранилища один раз.
  Кто вызывает: main.js.
  От чего зависит: lib/*, components/LockBanner.svelte, LiveBanner.svelte, pages/*.
  Настройки: нет.
-->
<script>
  import { connect, reconnectOnEvents } from './lib/live.svelte.js';
  import { start as startSettings } from './lib/settings.svelte.js';
  import { start as startSecrets } from './lib/secrets.svelte.js';
  import { start as startDex } from './lib/dex.svelte.js';
  import { start as startControl } from './lib/control.svelte.js';
  import { route, href } from './lib/router.svelte.js';
  import LockBanner from './components/LockBanner.svelte';
  import LiveBanner from './components/LiveBanner.svelte';
  import ControlBar from './components/ControlBar.svelte';
  import Unlock from './pages/Unlock.svelte';
  import Venues from './pages/Venues.svelte';
  import Settings from './pages/Settings.svelte';
  import Rules from './pages/Rules.svelte';
  import Live from './pages/Live.svelte';
  import Deals from './pages/Deals.svelte';
  import Coins from './pages/Coins.svelte';
  import Manual from './pages/Manual.svelte';
  import Channels from './pages/Channels.svelte';
  import GasStats from './pages/GasStats.svelte';
  import Notifications from './pages/Notifications.svelte';
  import Journal from './pages/Journal.svelte';

  /**
   * Экраны в меню: адрес, название, компонент. Первый — открывается, если адрес пустой.
   * wide — экран во всю ширину окна (широкие таблицы, Issue #33); остальные — колонкой по центру.
   */
  const PAGES = [
    { id: 'unlock', title: 'Разблокировка', page: Unlock },
    { id: 'venues', title: 'Площадки', page: Venues },
    { id: 'live', title: 'Живой экран', page: Live, wide: true },
    { id: 'deals', title: 'Сделки', page: Deals },
    { id: 'coins', title: 'Монеты', page: Coins, wide: true },
    { id: 'manual', title: 'Ручное управление', page: Manual },
    { id: 'channels', title: 'Каналы отправки', page: Channels },
    { id: 'gas', title: 'Статистика газа', page: GasStats },
    { id: 'settings', title: 'Настройки', page: Settings },
    { id: 'rules', title: 'Правила шлюза', page: Rules },
    { id: 'notify', title: 'Уведомления', page: Notifications },
    { id: 'journal', title: 'Журнал', page: Journal },
  ];

  startSettings();
  startSecrets();
  startDex();
  startControl();
  connect();
  reconnectOnEvents();

  const current = $derived(PAGES.find((p) => p.id === route.page) ?? PAGES[0]);
</script>

<nav>
  <strong class="brand">ARB</strong>
  {#each PAGES as p (p.id)}
    <a href={href(p.id)} class:active={p === current}>{p.title}</a>
  {/each}
</nav>
<LockBanner />
<LiveBanner />
<ControlBar />
<main class:wide={current.wide}>
  {#key current.id}<current.page />{/key}
</main>
