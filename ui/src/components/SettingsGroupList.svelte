<!--
  Файл: ui/src/components/SettingsGroupList.svelte
  Что делает: боковой список категорий экрана «Настройки» — переключение категорий без возврата в меню:
    «Все категории» и каждая категория схемы с числом настроек и числом не заданных нигде.
    Ссылки — адреса #/settings/<категория>, поэтому работает кнопка «Назад» браузера.
    На узком экране список становится строкой-переносом над настройками.
  Кто вызывает: pages/Settings.svelte.
  От чего зависит: lib/router.svelte.js (href), lib/settingsGroups.js (элементы groupStats).
  Настройки: нет.
-->
<script>
  import { href } from '../lib/router.svelte.js';

  /** stats — [{group, total, unset}]; current — открытая категория ('' — все категории). */
  let { stats, current } = $props();
</script>

<aside>
  <a href={href('settings')} class:active={!current}>Все категории</a>
  {#each stats as g (g.group)}
    <a href={href('settings', g.group)} class:active={current === g.group}>
      <span>{g.group}</span>
      <span class="cnt">{g.total}{#if g.unset}{' · '}<span class="unset-n">не задано {g.unset}</span>{/if}</span>
    </a>
  {/each}
</aside>

<style>
  aside {
    display: flex; flex-direction: column; gap: 1px; position: sticky; top: 56px; align-self: start;
    background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 4px 0;
    max-height: calc(100vh - 72px); overflow-y: auto;
  }
  a {
    display: flex; flex-direction: column; padding: 5px 12px; text-decoration: none; color: var(--text);
    border-left: 3px solid transparent;
  }
  a:hover { background: var(--bg); }
  a.active { border-left-color: var(--accent); background: var(--bg); font-weight: 600; }
  .cnt { font-size: 12px; color: var(--muted); font-weight: 400; }
  .unset-n { color: var(--unset); }
  @media (max-width: 760px) {
    aside { flex-direction: row; flex-wrap: wrap; position: static; max-height: none; gap: 4px; padding: 6px; }
    a { border: 1px solid var(--line); border-radius: 6px; padding: 3px 8px; }
    a.active { border-color: var(--accent); }
  }
</style>
