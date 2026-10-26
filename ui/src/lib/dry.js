// Файл: ui/src/lib/dry.js
// Что делает: состояние сухого режима (решение 74) по заданным значениям настроек dry.enabled и dry.budget_usdt
//   (store.values из lib/settings.svelte.js — обновляются вживую). Ядро читает те же настройки на каждом кандидате.
// Кто вызывает: components/DryBanner.svelte.
// От чего зависит: lib/settings.svelte.js (valueKey).
// Настройки: dry.enabled, dry.budget_usdt (только чтение).

import { valueKey } from './settings.svelte.js';

/**
 * Что делает: состояние сухого режима.
 * Принимает: store.values. Возвращает: {state: 'on' | 'nobudget' | 'off' | 'unset', budget}.
 *   unset — dry.enabled не задана (обычный режим); nobudget — включён без dry.budget_usdt (сигналов нет).
 */
export function dryState(values) {
  const on = values[valueKey('dry.enabled', { kind: 'Global' })];
  const budget = values[valueKey('dry.budget_usdt', { kind: 'Global' })];
  if (on === undefined) return { state: 'unset', budget };
  if (on !== true) return { state: 'off', budget };
  return { state: budget === undefined ? 'nobudget' : 'on', budget };
}
