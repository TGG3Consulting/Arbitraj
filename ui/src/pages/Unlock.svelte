<!--
  Файл: ui/src/pages/Unlock.svelte
  Что делает: экран «Разблокировка» — состояние секретов (GET /api/secrets/status); нет файла — создание
    с паролем и подтверждением (POST /api/secrets/create); разблокировка (POST /api/secrets/unlock);
    блокировка (POST /api/secrets/lock); известные секреты (GET /api/secrets/known) с вводом и удалением;
    смена пароля (POST /api/secrets/password). Пароли и значения не хранятся: после отправки поля очищаются.
    Адрес «#/unlock/<имя секрета>» выделяет этот секрет (переход с экранов «Уведомления», «Площадки»).
  Кто вызывает: App.svelte.
  От чего зависит: lib/api.js, lib/secrets.svelte.js, lib/router.svelte.js, components/SecretRow.svelte, ErrorText.svelte.
  Настройки: security.* проверяет бэкенд; не заданы — он вернёт причину, она показывается как есть.
-->
<script>
  import { tick } from 'svelte';
  import { post } from '../lib/api.js';
  import { secrets, refresh } from '../lib/secrets.svelte.js';
  import { route } from '../lib/router.svelte.js';
  import SecretRow from '../components/SecretRow.svelte';
  import ErrorText from '../components/ErrorText.svelte';

  let pw = $state('');
  let pw2 = $state('');
  let oldPw = $state('');
  let error = $state('');
  let pwError = $state('');
  let pwDone = $state('');
  let busy = $state(false);

  const status = $derived(secrets.status);
  /** Секреты, введённые, но не из списка известных — тоже показываются, чтобы их можно было удалить. */
  const extra = $derived(
    (status?.names ?? [])
      .filter((n) => !secrets.known.some((k) => k.name === n))
      .map((n) => ({ name: n, scope: '', description: 'нет в списке известных секретов' })),
  );

  /** Выполняет запрос; ошибка — в поле err. Пароли очищаются в любом случае. */
  async function act(fn, setErr) {
    busy = true;
    setErr('');
    try {
      await fn();
    } catch (e) {
      setErr(e.message);
    }
    pw = pw2 = oldPw = '';
    busy = false;
    refresh();
  }

  const create = () =>
    pw !== pw2
      ? (error = 'Пароль и подтверждение не совпадают')
      : act(() => post('/api/secrets/create', { password: pw }), (e) => (error = e));
  const unlock = () => act(() => post('/api/secrets/unlock', { password: pw }), (e) => (error = e));
  const lock = () => act(() => post('/api/secrets/lock'), (e) => (error = e));
  async function changePassword() {
    pwDone = '';
    if (pw !== pw2) return (pwError = 'Новый пароль и подтверждение не совпадают');
    await act(() => post('/api/secrets/password', { old: oldPw, new: pw }), (e) => (pwError = e));
    if (!pwError) pwDone = 'Пароль изменён';
  }

  // Переход к секрету по адресу «#/unlock/<имя>».
  $effect(() => {
    const name = route.arg;
    if (name && secrets.known.length) tick().then(() => document.getElementById(`secret-${name}`)?.scrollIntoView());
  });
</script>

<h1>Разблокировка</h1>
{#if !status}
  <p class="muted">Загружаю состояние секретов…</p>
{:else if !status.file_exists}
  <section class="card">
    <h2>Создать файл секретов</h2>
    <p>Пароль нигде не сохраняется. Без него ключи не расшифровать — запомните его.</p>
    <form class="row" onsubmit={(e) => { e.preventDefault(); create(); }}>
      <input type="password" autocomplete="new-password" bind:value={pw} placeholder="пароль" />
      <input type="password" autocomplete="new-password" bind:value={pw2} placeholder="ещё раз" />
      <button disabled={busy || !pw || !pw2}>Создать</button>
    </form>
    <ErrorText text={error} />
  </section>
{:else if !status.unlocked}
  <section class="card">
    <h2>Разблокировать</h2>
    <form class="row" onsubmit={(e) => { e.preventDefault(); unlock(); }}>
      <input type="password" autocomplete="current-password" bind:value={pw} placeholder="пароль" />
      <button disabled={busy || !pw}>Разблокировать</button>
    </form>
    <ErrorText text={error} />
  </section>
{:else}
  <section class="card row">
    <span class="ok">Секреты разблокированы — ключи в памяти.</span>
    <button onclick={lock} disabled={busy}>Заблокировать</button>
    <ErrorText text={error} />
  </section>
{/if}

{#if status?.file_exists}
  <h2>Секреты</h2>
  <p class="muted">Значения не показываются никогда. Для сохранения и удаления нужен пароль.</p>
  {#each [...secrets.known, ...extra] as item (item.name)}
    <SecretRow {item} highlight={route.arg === item.name} />
  {/each}

  <section class="card">
    <h2>Сменить пароль</h2>
    <form class="row" onsubmit={(e) => { e.preventDefault(); changePassword(); }}>
      <input type="password" autocomplete="current-password" bind:value={oldPw} placeholder="старый пароль" />
      <input type="password" autocomplete="new-password" bind:value={pw} placeholder="новый пароль" />
      <input type="password" autocomplete="new-password" bind:value={pw2} placeholder="новый ещё раз" />
      <button disabled={busy || !oldPw || !pw || !pw2}>Сменить</button>
      {#if pwDone}<span class="ok">{pwDone}</span>{/if}
    </form>
    <ErrorText text={pwError} />
  </section>
{/if}
