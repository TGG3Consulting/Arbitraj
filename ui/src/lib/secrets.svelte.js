// Файл: ui/src/lib/secrets.svelte.js
// Что делает: состояние секретов для всех экранов — создан ли файл, разблокировано ли, имена введённых
//   секретов (GET /api/secrets/status) и список известных секретов (GET /api/secrets/known).
//   Значения секретов сюда не попадают никогда: API их не отдаёт, интерфейс не хранит.
//   Обновляется после каждого действия и по записям журнала вида «secrets» из живого потока
//   (так изменения из другой вкладки видны сразу).
// Кто вызывает: App.svelte (start), components/LockBanner.svelte, SecretRow.svelte, экраны.
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/flight.js (один запрос в полёте).
// Настройки: нет.

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';

/** Имя секрета токена Telegram (src/secrets/mod.rs, TELEGRAM_BOT_TOKEN) — экраны «Уведомления» и «Площадки». */
export const TELEGRAM_TOKEN = 'telegram.bot_token';

/**
 * Решение 176: роли чатов Telegram (src/notify/roles.rs) — код роли (POST /api/check/telegram/{code}), название, настройка
 * ID чата, секрет токена, настройка разрешённых пользователей (null — у роли нет кнопок).
 */
export const TELEGRAM_ROLES = [
  { code: 'control', title: 'Управление', chat: 'notify.control_chat', token: 'telegram.control_token', users: 'notify.control_users' },
  { code: 'general', title: 'Общий', chat: 'notify.telegram_chat', token: TELEGRAM_TOKEN, users: 'notify.general_users' },
  { code: 'deals', title: 'Сделки', chat: 'notify.deals_chat', token: 'telegram.deals_token', users: null },
  { code: 'failures', title: 'Неудачные', chat: 'notify.failures_chat', token: 'telegram.failures_token', users: 'notify.failures_users' },
];

/** status: {file_exists, unlocked, names} или null — ещё не загружено; known: [{name, scope, description}]. */
export const secrets = $state({ status: null, known: [], error: '' });

/** Что делает: перечитывает состояние и список (один запрос в полёте — lib/flight.js). Ошибка — в secrets.error. */
export const refresh = flight(async () => {
  try {
    const [status, known] = await Promise.all([get('/api/secrets/status'), get('/api/secrets/known')]);
    secrets.status = status;
    secrets.known = known;
    secrets.error = '';
  } catch (e) {
    secrets.error = e.message;
  }
});

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  refresh();
  onLive((msg) => {
    if (lagFor(msg, 'journal') || (msg.type === 'journal' && msg.data.kind === 'secrets')) refresh();
  });
}
