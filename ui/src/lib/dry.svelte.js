// Файл: ui/src/lib/dry.svelte.js
// Что делает: сухой режим для плашки (решение 74) — как его видит ядро: GET /api/dry ({state, budget, reason}),
//   интерфейс сам режим не вычисляет. Перечитывается при изменении настроек dry.* (живое сообщение settings) и при lagged.
// Кто вызывает: App.svelte (start), components/DryBanner.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: dry.enabled, dry.budget_usdt (только показ).

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';

/** data — {state: 'on'|'nobudget'|'broken'|'off'|'unset', budget, reason} или null; error — сбой запроса. */
export const dry = $state({ data: null, error: '' });

/** Перечитывает режим. Ошибка — в dry.error. */
async function reload() {
  try {
    dry.data = await get('/api/dry');
    dry.error = '';
  } catch (e) {
    dry.error = e.message;
  }
}

/** Первая загрузка и подписка (один раз из App.svelte). */
export function start() {
  reload();
  onLive((msg) => {
    if (lagFor(msg, 'settings') || (msg.type === 'settings' && msg.data?.name?.startsWith('dry.'))) reload();
  });
}
