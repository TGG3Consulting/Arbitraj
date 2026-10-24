<!--
  Файл: ui/src/components/NavDropdown.svelte
  Что делает: пункт меню с выпадающим списком ссылок (меню «Настройки» по категориям, «Ещё»).
    Открывается щелчком; закрывается выбором пункта, повторным щелчком, клавишей Esc и щелчком мимо списка.
  Кто вызывает: components/NavMenu.svelte.
  От чего зависит: ничего.
  Настройки: нет.
-->
<script>
  /**
   * title — подпись пункта; active — подсветить пункт (открыт один из его экранов);
   * items — [{href, label, note?, active?}]: note — приглушённая приписка (например «не задано 2»).
   */
  let { title, active = false, items } = $props();
  let open = $state(false);
  let root;

  /** Щелчок мимо выпадающего списка — закрыть. */
  function onDoc(e) {
    if (open && root && !root.contains(e.target)) open = false;
  }
  /** Esc — закрыть. */
  function onKey(e) {
    if (e.key === 'Escape') open = false;
  }
</script>

<svelte:window onclick={onDoc} onkeydown={onKey} />

<span class="dd" bind:this={root}>
  <button class="link" class:active aria-haspopup="true" aria-expanded={open} onclick={() => (open = !open)}>
    {title} <span class="caret">▾</span>
  </button>
  {#if open}
    <div class="menu" role="menu">
      {#each items as it (it.href)}
        <a role="menuitem" href={it.href} class:active={it.active} onclick={() => (open = false)}>
          <span>{it.label}</span>{#if it.note}<span class="note">{it.note}</span>{/if}
        </a>
      {/each}
    </div>
  {/if}
</span>

<style>
  .dd { position: relative; display: inline-block; }
  .link {
    background: transparent; color: var(--muted); padding: 4px 0; border-radius: 0;
    border-bottom: 2px solid transparent;
  }
  .link.active { color: var(--text); border-color: var(--accent); }
  .caret { font-size: 11px; }
  .menu {
    position: absolute; top: calc(100% + 6px); left: 0; z-index: 5; min-width: 260px; max-height: 75vh;
    overflow-y: auto; background: var(--panel); border: 1px solid var(--line); border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18); padding: 4px 0;
  }
  .menu a {
    display: flex; justify-content: space-between; gap: 16px; padding: 6px 12px; color: var(--text);
    text-decoration: none; white-space: nowrap; border: 0;
  }
  .menu a:hover { background: var(--bg); }
  .menu a.active { color: var(--accent); font-weight: 600; }
  .note { color: var(--muted); font-size: 13px; }
</style>
