// Файл: ui/src/lib/remote.svelte.js
// Что делает: данные одного экрана из одного GET-маршрута с перечитыванием по живому потоку.
//   Ответ 503 — «нет данных» (источник ещё не подключён, src/api/README.md): показывается причиной бэкенда,
//   а не пустым списком. Любая другая ошибка — как есть. Перечитывается по «lagged» и по указанным
//   типам живых сообщений (например coin_check) — без таймеров и опроса. Один запрос в полёте (lib/flight.js, шторм
//   запросов 10.11.2026): поток сообщений во время запроса — один повтор после него; уход со страницы — отмена.
// Кто вызывает: экраны «Монеты», «Ручное управление», «Статистика газа» (в блоке <script> компонента).
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/flight.js.
// Настройки: нет.

import { get } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';

/**
 * Что делает: создаёт состояние данных и подписку (живёт, пока жив компонент).
 * Принимает: путь GET, список типов живых сообщений, по которым перечитать.
 * Возвращает: $state {data, noData, error, loading, reload} — data = null, пока данных нет.
 */
export function remote(path, liveTypes) {
  /** Перечитывает данные (signal — отмена). Ошибки — в noData (503) или error; отмена — ничего. */
  async function load(signal) {
    try {
      st.data = await get(path, signal);
      st.noData = '';
      st.error = '';
    } catch (e) {
      if (e.aborted) return;
      st.data = null;
      st.noData = e.status === 503 ? e.message : '';
      st.error = e.status === 503 ? '' : e.message;
    }
    st.loading = false;
  }

  const reload = flight(load);
  const st = $state({ data: null, noData: '', error: '', loading: true, reload });

  $effect(() => {
    reload();
    const off = onLive((msg) => {
      if (lagFor(msg, 'core') || liveTypes.includes(msg.type)) reload();
    });
    return () => {
      off();
      reload.cancel();
    };
  });
  return st;
}
