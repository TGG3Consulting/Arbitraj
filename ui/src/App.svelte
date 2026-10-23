<!--
  Файл: ui/src/App.svelte
  Что делает: каркас интерфейса — меню экранов, плашки «Система заблокирована» и «Нет живой связи»,
    выбор экрана по адресу (#/экран). Запускает живой поток и общие хранилища один раз.
  Кто вызывает: main.js.
  От чего зависит: lib/*, components/LockBanner.svelte, LiveBanner.svelte, pages/*.
  Настройки: нет.
-->
<script>
  import { connect, reconnectOnEvents } from './lib/live.svelte.js';
  import { start as startSettings } from './lib/settings.svelte.js';
  import { start as startSecrets } from './lib/secrets.svelte.js';
  import { start as startDex } from './lib/dex.svelte.js';
  import { route, href } from './lib/router.svelte.js';
  import LockBanner from './components/LockBanner.svelte';
  import LiveBanner from './components/LiveBanner.svelte';
  import Unlock from './pages/Unlock.svelte';
  import Venues from './pages/Venues.svelte';
  import Settings from './pages/Settings.svelte';
  import Rules from './pages/Rules.svelte';
  import Notifications from './pages/Notifications.svelte';
  import Journal from './pages/Journal.svelte';

  /** Экраны в меню: адрес, название, компонент. Первый — открывается, если адрес пустой. */
  const PAGES = [
    { id: 'unlock', title: 'Разблокировка', page: Unlock },
    { id: 'venues', title: 'Площадки', page: Venues },
    { id: 'settings', title: 'Настройки', page: Settings },
    { id: 'rules', title: 'Правила шлюза', page: Rules },
    { id: 'notify', title: 'Уведомления', page: Notifications },
    { id: 'journal', title: 'Журнал', page: Journal },
  ];

  startSettings();
  startSecrets();
  startDex();
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
<main>
  {#key current.id}<current.page />{/key}
</main>
