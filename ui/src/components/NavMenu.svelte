<!--
  Файл: ui/src/components/NavMenu.svelte
  Что делает: верхнее меню экранов. Обычные экраны — ссылками; «Настройки» — выпадающий список категорий
    из схемы (поле group) с числом настроек и числом не заданных нигде, плюс «Все категории и поиск»;
    редкие экраны (флаг more у экрана в App.svelte) — в выпадающем «Ещё», чтобы меню умещалось в одну строку.
  Кто вызывает: App.svelte.
  От чего зависит: lib/router.svelte.js, lib/settings.svelte.js, lib/settingsGroups.js, components/NavDropdown.svelte.
  Настройки: нет.
-->
<script>
  import { route, href } from '../lib/router.svelte.js';
  import { store } from '../lib/settings.svelte.js';
  import { groupStats } from '../lib/settingsGroups.js';
  import NavDropdown from './NavDropdown.svelte';

  /** pages — экраны [{id, title, more?, menu?}]; current — открытый экран. */
  let { pages, current } = $props();

  const main = $derived(pages.filter((p) => !p.more));
  const more = $derived(pages.filter((p) => p.more));
  /** Пункты «Настройки»: все категории и каждая категория со счётчиками. */
  const settingsItems = $derived([
    { href: href('settings'), label: 'Все категории и поиск', active: current.id === 'settings' && !route.arg },
    ...groupStats(store.schema, store.values).map((g) => ({
      href: href('settings', g.group),
      label: g.group,
      note: `${g.total}${g.unset ? `, не задано ${g.unset}` : ''}`,
      active: current.id === 'settings' && route.arg === g.group,
    })),
  ]);
  const moreItems = $derived(more.map((p) => ({ href: href(p.id), label: p.title, active: p === current })));
</script>

<nav>
  <strong class="brand">ARB</strong>
  {#each main as p (p.id)}
    {#if p.menu === 'settings'}
      <NavDropdown title={p.title} active={p === current} items={settingsItems} />
    {:else}
      <a href={href(p.id)} class:active={p === current}>{p.title}</a>
    {/if}
  {/each}
  {#if more.length}
    <NavDropdown title="Ещё" active={more.includes(current)} items={moreItems} />
  {/if}
</nav>
