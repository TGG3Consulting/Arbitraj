<!--
  Файл: ui/src/components/Modal.svelte
  Что делает: всплывающее (модальное) окно поверх экрана с заголовком и крестиком (Issue #33).
    Закрывается крестиком, клавишей Esc и щелчком мимо окна. Содержимое — то, что передал экран.
  Кто вызывает: components/LiveCoinModal.svelte, components/CoinModal.svelte.
  От чего зависит: ничего.
  Настройки: нет.
-->
<script>
  /** title — заголовок окна; onclose() — закрыть; children — содержимое окна. */
  let { title, onclose, children } = $props();

  /** Esc — закрыть. Принимает: событие клавиатуры. */
  function onkey(e) {
    if (e.key === 'Escape') onclose();
  }
  /** Щелчок по затемнению (не по самому окну) — закрыть. */
  function onback(e) {
    if (e.target === e.currentTarget) onclose();
  }
</script>

<svelte:window onkeydown={onkey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onback}>
  <div class="modal" role="dialog" aria-modal="true" aria-label={title}>
    <header>
      <h2>{title}</h2>
      <button class="close" aria-label="Закрыть" title="Закрыть (Esc)" onclick={onclose}>×</button>
    </header>
    <div class="body">{@render children()}</div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed; inset: 0; z-index: 10; background: rgba(0, 0, 0, 0.45);
    display: flex; align-items: flex-start; justify-content: center; padding: 4vh 16px; overflow-y: auto;
  }
  .modal {
    background: var(--panel); color: var(--text); border: 1px solid var(--line); border-radius: 10px;
    width: min(1100px, 100%); box-shadow: 0 10px 40px rgba(0, 0, 0, 0.35);
  }
  header {
    display: flex; align-items: center; justify-content: space-between; gap: 12px;
    padding: 10px 14px; border-bottom: 1px solid var(--line); position: sticky; top: 0; background: var(--panel);
    border-radius: 10px 10px 0 0;
  }
  h2 { margin: 0; }
  .close { background: transparent; color: var(--muted); font-size: 24px; line-height: 1; padding: 2px 10px; }
  .close:hover { color: var(--text); }
  .body { padding: 10px 14px 16px; }
</style>
