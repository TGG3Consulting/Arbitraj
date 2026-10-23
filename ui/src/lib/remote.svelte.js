// Файл: ui/src/lib/remote.svelte.js
// Что делает: данные одного экрана из одного GET-маршрута с перечитыванием по живому потоку.
//   Ответ 503 — «нет данных» (источник ещё не подключён, src/api/README.md): показывается причиной бэкенда,
//   а не пустым списком. Любая другая ошибка — как есть. Перечитывается по «lagged» и по указанным
//   типам живых сообщений (например coin_check) — без таймеров и опроса.
// Кто вызывает: экраны «Монеты», «Ручное управление», «Статистика газа» (в блоке <script> компонента).
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: нет.

import { get } from './api.js';
import { onLive } from './live.svelte.js';

/**
 * Что делает: создаёт состояние данных и подписку (живёт, пока жив компонент).
 * Принимает: путь GET, список типов живых сообщений, по которым перечитать.
 * Возвращает: $state {data, noData, error, loading, reload} — data = null, пока данных нет.
 */
export function remote(path, liveTypes) {
  const st = $state({ data: null, noData: '', error: '', loading: true, reload });

  /** Перечитывает данные. Ошибки — в noData (503) или error. */
  async function reload() {
    try {
      st.data = await get(path);
      st.noData = '';
      st.error = '';
    } catch (e) {
      st.data = null;
      st.noData = e.status === 503 ? e.message : '';
      st.error = e.status === 503 ? '' : e.message;
    }
    st.loading = false;
  }

  $effect(() => {
    reload();
    return onLive((msg) => {
      if (msg.type === 'lagged' || liveTypes.includes(msg.type)) reload();
    });
  });
  return st;
}
