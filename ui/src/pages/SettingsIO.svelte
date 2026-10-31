<!--
  Файл: ui/src/pages/SettingsIO.svelte
  Что делает: экран «Импорт/экспорт настроек». Экспорт двумя кнопками: «Экспортировать настройки»
    (только TOML) и «Экспортировать с секретами» (полный бэкап — настройки + ЗАШИФРОВАННОЕ хранилище
    секретов как есть; открытым текстом секретов в файле нет, но утечка файла = риск потери ключей).
    Импорт: выбор файла (чистые настройки ИЛИ полный бэкап) → отчёт: сколько применено, что отклонено
    и почему, установилось ли зашифрованное хранилище секретов. После импорта настройки живые и видны
    на «Настройках». Применяются настройки только через хранилище (журнал изменений, обновление на лету).
  Кто вызывает: App.svelte.
  От чего зависит: lib/settingsIo.svelte.js, components/ErrorText.svelte, lib/router.svelte.js (ссылка на «Настройки»).
  Настройки: нет своих (работает со всеми настройками каталога).
-->
<script>
  import { io, exportSettings, importFile } from '../lib/settingsIo.svelte.js';
  import { href } from '../lib/router.svelte.js';
  import ErrorText from '../components/ErrorText.svelte';

  /** Выбор файла → импорт (значение сбрасывается, чтобы повторный выбор того же файла сработал). */
  function onPick(e) {
    const file = e.currentTarget.files?.[0];
    if (file) importFile(file);
    e.currentTarget.value = '';
  }
</script>

<h1>Импорт/экспорт настроек</h1>
<p class="muted">
  Залить все настройки разом одним файлом или сделать бэкап/перенос — вместо ввода по одной.
  Настройки применяются через хранилище: с журналом изменений и обновлением на лету.
</p>
<ErrorText text={io.error} />

<section class="block">
  <h2>Экспорт</h2>
  <p class="muted">Скачивает текущие заданные настройки в файл TOML (имя, область, значение).</p>
  <div class="row">
    <button onclick={() => exportSettings(false)} disabled={io.exporting}>Экспортировать настройки</button>
    <button class="secrets" onclick={() => exportSettings(true)} disabled={io.exporting}>Экспортировать с секретами</button>
  </div>
  <div class="warn">
    <strong>«Экспортировать с секретами»</strong> добавляет в файл ЗАШИФРОВАННОЕ хранилище секретов как есть
    (приватный ключ кошелька и ключи биржи). Открытым текстом секретов в файле нет, но файл-бэкап =
    ваши ключи: храните его в безопасности, не пересылайте по открытым каналам. Расшифровать его можно
    только вашим паролем хранилища.
  </div>
</section>

<section class="block">
  <h2>Импорт</h2>
  <p class="muted">
    Выберите файл — чистые настройки или полный бэкап. Каждая настройка проверяется и применяется
    по отдельности; негодные попадают в отчёт с причиной, остальные применяются. Если в файле есть
    зашифрованное хранилище секретов — оно устанавливается, и его нужно разблокировать исходным паролем.
  </p>
  <div class="row">
    <input type="file" accept=".toml,text/plain,application/toml" onchange={onPick} />
    {#if io.importing}<span class="muted">Импортирую…</span>{/if}
  </div>

  {#if io.report}
    <div class="report">
      <p class="done">
        Файл «{io.fileName}»: применено <strong>{io.report.applied.length}</strong>,
        отклонено <strong>{io.report.failed.length}</strong>.
      </p>

      {#if io.report.secrets_installed}
        <p class="ok">Зашифрованное хранилище секретов установлено. Разблокируйте его исходным паролем на экране «Разблокировка».</p>
      {:else if io.report.secrets_error}
        <p class="err">Хранилище секретов не установлено: {io.report.secrets_error}</p>
      {/if}

      {#if io.report.failed.length}
        <h3>Отклонены</h3>
        <table>
          <thead><tr><th>Настройка</th><th>Область</th><th>Причина</th></tr></thead>
          <tbody>
            {#each io.report.failed as f (f.name + '|' + f.scope)}
              <tr><td><code>{f.name}</code></td><td>{f.scope}</td><td>{f.reason}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}

      {#if io.report.applied.length}
        <details>
          <summary>Применённые настройки ({io.report.applied.length})</summary>
          <table>
            <thead><tr><th>Настройка</th><th>Область</th></tr></thead>
            <tbody>
              {#each io.report.applied as a (a.name + '|' + a.scope)}
                <tr><td><code>{a.name}</code></td><td>{a.scope}</td></tr>
              {/each}
            </tbody>
          </table>
        </details>
      {/if}

      <p class="muted"><a href={href('settings')}>Открыть «Настройки»</a> — применённые значения уже там.</p>
    </div>
  {/if}
</section>

<style>
  .block { margin-bottom: 24px; }
  .row { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin: 8px 0; }
  .secrets { border-color: var(--line, #c9b458); }
  .warn { border: 1px solid var(--line, #c9b458); background: color-mix(in srgb, #c9b458 12%, transparent); border-radius: 8px; padding: 10px 14px; margin-top: 8px; }
  .report { margin-top: 12px; }
  .done { margin: 6px 0; }
  .ok { color: var(--ok, #2e7d32); }
  table { border-collapse: collapse; margin: 8px 0; width: 100%; }
  th, td { text-align: left; padding: 4px 8px; border-bottom: 1px solid var(--line, #ddd); vertical-align: top; }
  details { margin: 8px 0; }
  summary { cursor: pointer; }
</style>
