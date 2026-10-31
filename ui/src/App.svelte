<!--
  Файл: ui/src/App.svelte
  Что делает: каркас интерфейса — меню экранов (NavMenu: «Настройки» по категориям, редкие экраны в «Ещё»), плашки «Система заблокирована», «Нет живой связи»
    и режима системы («Старт/Стоп/Пауза»), плашка сухого режима (решение 74, DryBanner — когда включён),
    выбор экрана по адресу (#/экран); «Живой экран» и «Монеты» — во всю ширину окна (Issue #33). Запускает живой поток и общие хранилища один раз.
  Кто вызывает: main.js.
  От чего зависит: lib/*, components/LockBanner.svelte, LiveBanner.svelte, ControlBar.svelte, DryBanner.svelte, pages/*.
  Настройки: нет.
-->
<script>
  import { connect, reconnectOnEvents } from './lib/live.svelte.js';
  import { start as startSettings } from './lib/settings.svelte.js';
  import { start as startSecrets } from './lib/secrets.svelte.js';
  import { start as startDex } from './lib/dex.svelte.js';
  import { start as startControl } from './lib/control.svelte.js';
  import { start as startDry } from './lib/dry.svelte.js';
  import { route } from './lib/router.svelte.js';
  import LockBanner from './components/LockBanner.svelte';
  import LiveBanner from './components/LiveBanner.svelte';
  import ControlBar from './components/ControlBar.svelte';
  import DryBanner from './components/DryBanner.svelte';
  import NavMenu from './components/NavMenu.svelte';
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
  import Episodes from './pages/Episodes.svelte';
  import FirstRun from './pages/FirstRun.svelte';
  import SettingsIO from './pages/SettingsIO.svelte';

  /**
   * Экраны в меню: адрес, название, компонент. Первый — открывается, если адрес пустой.
   * wide — экран во всю ширину окна (широкие таблицы, Issue #33); остальные — колонкой по центру.
   * more — редкий экран: в меню он в выпадающем «Ещё»; menu: 'settings' — пункт с выпадающим списком категорий.
   */
  const PAGES = [
    { id: 'unlock', title: 'Разблокировка', page: Unlock },
    { id: 'venues', title: 'Площадки', page: Venues },
    { id: 'live', title: 'Живой экран', page: Live, wide: true },
    { id: 'deals', title: 'Сделки', page: Deals },
    { id: 'coins', title: 'Монеты', page: Coins, wide: true },
    { id: 'episodes', title: 'Эпизоды', page: Episodes, wide: true },
    { id: 'manual', title: 'Ручное управление', page: Manual },
    { id: 'channels', title: 'Каналы отправки', page: Channels, more: true },
    { id: 'gas', title: 'Статистика газа', page: GasStats, more: true },
    { id: 'settings', title: 'Настройки', page: Settings, menu: 'settings' },
    { id: 'settings-io', title: 'Импорт/экспорт настроек', page: SettingsIO, more: true },
    { id: 'first-run', title: 'Первый запуск', page: FirstRun, more: true },
    { id: 'rules', title: 'Правила шлюза', page: Rules, more: true },
    { id: 'notify', title: 'Уведомления', page: Notifications, more: true },
    { id: 'journal', title: 'Журнал', page: Journal },
  ];

  startSettings();
  startSecrets();
  startDex();
  startControl();
  startDry();
  connect();
  reconnectOnEvents();

  const current = $derived(PAGES.find((p) => p.id === route.page) ?? PAGES[0]);
</script>

<NavMenu pages={PAGES} {current} />
<LockBanner />
<LiveBanner />
<ControlBar />
<DryBanner />
<main class:wide={current.wide}>
  {#key current.id}<current.page />{/key}
</main>
