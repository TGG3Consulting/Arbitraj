<!--
  Файл: ui/src/components/SecretRow.svelte
  Что делает: один секрет — имя, описание, введён ли (видно только при разблокировке), ввод значения
    и пароля для сохранения (PUT /api/secrets/{name} {password, value}), удаление (DELETE {password}).
    Значение никогда не показывается и не хранится в браузере: поля type=password, без автозаполнения,
    после отправки поля очищаются. Ошибка бэкенда — как есть.
  Кто вызывает: pages/Unlock.svelte, pages/Venues.svelte.
  От чего зависит: lib/api.js, lib/secrets.svelte.js, components/ErrorText.svelte.
  Настройки: нет (длину пароля и параметры шифрования проверяет бэкенд).
-->
<script>
  import { put, del, seg } from '../lib/api.js';
  import { secrets, refresh } from '../lib/secrets.svelte.js';
  import ErrorText from './ErrorText.svelte';

  /** item — {name, scope, description}; highlight — выделить (переход по ссылке). */
  let { item, highlight = false } = $props();
  let value = $state('');
  let password = $state('');
  let error = $state('');
  let done = $state('');
  let busy = $state(false);

  const status = $derived(secrets.status);
  const entered = $derived(status?.unlocked ? status.names.includes(item.name) : null);

  /** Выполняет действие с секретом и очищает поля. Принимает: функцию запроса и текст успеха. */
  async function run(action, okText) {
    busy = true;
    error = '';
    done = '';
    try {
      await action();
      done = okText;
    } catch (e) {
      error = e.message;
    }
    value = '';
    password = '';
    busy = false;
    refresh();
  }

  const save = () => run(() => put(`/api/secrets/${seg(item.name)}`, { password, value }), 'Сохранено');
  const remove = () => run(() => del(`/api/secrets/${seg(item.name)}`, { password }), 'Удалено');
</script>

<div class="card secret" class:hl={highlight} id={`secret-${item.name}`}>
  <header>
    <code>{item.name}</code>
    <span class="muted">{item.description}</span>
    {#if entered === true}<span class="ok">введён</span>
    {:else if entered === false}<span class="unset">не введён</span>
    {:else}<span class="muted">введён ли — видно после разблокировки</span>{/if}
  </header>
  <form class="row" onsubmit={(e) => { e.preventDefault(); save(); }}>
    <input type="password" autocomplete="off" bind:value placeholder="новое значение" />
    <input type="password" autocomplete="current-password" bind:value={password} placeholder="пароль секретов" />
    <button disabled={busy || !value || !password}>Сохранить</button>
    <button type="button" class="quiet" disabled={busy || !password} onclick={remove}>Удалить</button>
    {#if done}<span class="ok">{done}</span>{/if}
  </form>
  <ErrorText text={error} />
</div>
