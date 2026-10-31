// Файл: ui/src/lib/settingsIo.svelte.js
// Что делает: импорт/экспорт всех настроек одним файлом (TOML). Два режима экспорта:
//   «Экспорт настроек» — только настройки (GET /api/settings/export); «Экспорт с секретами» —
//   полный бэкап (GET /api/settings/export?secrets=true): настройки + ЗАШИФРОВАННОЕ хранилище секретов
//   как есть (base64, без расшифровки). Импорт (POST /api/settings/import) принимает оба формата: сервер
//   сам видит секцию [secrets]. Показывает отчёт {applied, failed, secrets_installed, secrets_error} и
//   перечитывает хранилище настроек, чтобы изменения сразу были видны на «Настройках» (они и так приходят
//   живым потоком, reload — на случай отсутствия связи).
// Кто вызывает: pages/SettingsIO.svelte.
// От чего зависит: lib/api.js (get и разбор ошибок), lib/settings.svelte.js (reload после импорта).
// Настройки: нет своих.

import { get } from './api.js';
import { reload as reloadSettings } from './settings.svelte.js';

/** Состояние экрана: идут ли экспорт/импорт, ошибка, отчёт последнего импорта и имя файла. */
export const io = $state({ exporting: false, importing: false, error: '', report: null, fileName: '' });

/**
 * Что делает: скачивает настройки в файл; withSecrets — полный бэкап с зашифрованным хранилищем секретов.
 * Принимает: withSecrets (булево). Возвращает: ничего. Ошибки: кладёт текст в io.error.
 */
export async function exportSettings(withSecrets = false) {
  io.exporting = true;
  io.error = '';
  try {
    const path = withSecrets ? '/api/settings/export?secrets=true' : '/api/settings/export';
    const toml = await get(path);
    const name = withSecrets ? 'arb-backup.toml' : 'arb-settings.toml';
    const blob = new Blob([toml ?? ''], { type: 'application/toml' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = name;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  } catch (e) {
    io.error = e.message;
  } finally {
    io.exporting = false;
  }
}

/**
 * Что делает: отправляет содержимое файла на импорт, показывает отчёт и перечитывает настройки.
 *   Файл может быть чистыми настройками или полным бэкапом — сервер различает сам.
 * Принимает: File (из <input type="file">). Возвращает: ничего. Ошибки: кладёт текст в io.error.
 */
export async function importFile(file) {
  io.importing = true;
  io.error = '';
  io.report = null;
  io.fileName = file.name;
  try {
    const text = await file.text();
    const res = await fetch('/api/settings/import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/toml' },
      body: text,
    });
    const body = await res.text();
    let data = null;
    if (body) {
      try {
        data = JSON.parse(body);
      } catch {
        data = body;
      }
    }
    if (!res.ok) {
      const reason = data && typeof data === 'object' && 'error' in data ? data.error : data;
      throw new Error(reason || `Ошибка ${res.status}`);
    }
    io.report = data;
    await reloadSettings();
  } catch (e) {
    io.error = e.message;
  } finally {
    io.importing = false;
  }
}
