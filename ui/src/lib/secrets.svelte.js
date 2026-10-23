// Файл: ui/src/lib/secrets.svelte.js
// Что делает: состояние секретов для всех экранов — создан ли файл, разблокировано ли, имена введённых
//   секретов (GET /api/secrets/status) и список известных секретов (GET /api/secrets/known).
//   Значения секретов сюда не попадают никогда: API их не отдаёт, интерфейс не хранит.
//   Обновляется после каждого действия и по записям журнала вида «secrets» из живого потока
//   (так изменения из другой вкладки видны сразу).
// Кто вызывает: App.svelte (start), components/LockBanner.svelte, SecretRow.svelte, экраны.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: нет.

import { get } from './api.js';
import { onLive } from './live.svelte.js';

/** Имя секрета токена Telegram (src/secrets/mod.rs, TELEGRAM_BOT_TOKEN) — экраны «Уведомления» и «Площадки». */
export const TELEGRAM_TOKEN = 'telegram.bot_token';

/** status: {file_exists, unlocked, names} или null — ещё не загружено; known: [{name, scope, description}]. */
export const secrets = $state({ status: null, known: [], error: '' });

/** Что делает: перечитывает состояние и список. Ошибка — в secrets.error. */
export async function refresh() {
  try {
    const [status, known] = await Promise.all([get('/api/secrets/status'), get('/api/secrets/known')]);
    secrets.status = status;
    secrets.known = known;
    secrets.error = '';
  } catch (e) {
    secrets.error = e.message;
  }
}

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  refresh();
  onLive((msg) => {
    if (msg.type === 'lagged' || (msg.type === 'journal' && msg.data.kind === 'secrets')) refresh();
  });
}
