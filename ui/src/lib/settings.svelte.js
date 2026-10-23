// Файл: ui/src/lib/settings.svelte.js
// Что делает: общее для экранов хранилище — схема настроек и событий (GET /api/settings/schema),
//   заданные значения (GET /api/settings), список площадок (GET /api/venues) и их вид (GET /api/venues/kinds).
//   Значения обновляются на лету из живого потока ({"type":"settings"}), при «lagged» — перечитываются.
//   Значений по умолчанию нет: не задано — значит в values нет записи.
// Кто вызывает: App.svelte (start), экраны «Настройки», «Уведомления», «Площадки», components/SettingEditor.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js.
// Настройки: читает и показывает любые.

import { get, put } from './api.js';
import { onLive } from './live.svelte.js';

/** schema: {settings, events} или null; values: ключ области → значение; venues: id площадок;
 *  venueKinds: [{id, kind: 'Cex' | 'Dex'}]. */
export const store = $state({ schema: null, values: {}, venues: [], venueKinds: [], error: '' });

/**
 * Ключ значения в store.values.
 * Принимает: имя настройки, область {kind, id?}. Возвращает: строку «имя|вид|id».
 */
export const valueKey = (name, scope) => `${name}|${scope.kind}|${scope.id ?? ''}`;

/** Что делает: перечитывает всё по REST. Ошибка — в store.error. */
export async function reload() {
  try {
    const [schema, values, venues, venueKinds] = await Promise.all([
      get('/api/settings/schema'),
      get('/api/settings'),
      get('/api/venues'),
      get('/api/venues/kinds'),
    ]);
    const map = {};
    for (const v of values) map[valueKey(v.name, v.scope)] = v.value;
    store.schema = schema;
    store.values = map;
    store.venues = venues;
    store.venueKinds = venueKinds;
    store.error = '';
  } catch (e) {
    store.error = e.message;
  }
}

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  reload();
  onLive((msg) => {
    if (msg.type === 'lagged') reload();
    if (msg.type === 'settings') {
      const c = msg.data;
      const key = valueKey(c.name, c.scope);
      if (c.new === null || c.new === undefined) delete store.values[key];
      else store.values[key] = c.new;
    }
  });
}

/**
 * Что делает: меняет значение (PUT /api/settings). Хранилище обновится из живого потока
 *   и сразу — из ответа, если живой связи нет.
 * Принимает: имя, область {kind, id?}, значение (null — очистить). Возвращает: ничего.
 * Ошибки: Error с текстом бэкенда как есть.
 */
export async function setValue(name, scope, value) {
  const body = { name, scope: scope.kind === 'Global' ? { kind: 'Global' } : scope, value };
  const change = await put('/api/settings', body);
  if (change) {
    const key = valueKey(name, scope);
    if (change.new === null || change.new === undefined) delete store.values[key];
    else store.values[key] = change.new;
  }
}

/** Описание настройки по имени. Принимает: имя. Возвращает: описание или undefined. */
export const defOf = (name) => store.schema?.settings.find((d) => d.name === name);

/**
 * Подпись области «площадка» у настройки. У настроек с venue_role «TransferSource» (решение 51) площадка —
 * источник перевода: биржа → «bingx → кошелёк», DEX (кошелёк) → «кошелёк → <биржи>».
 * Принимает: описание настройки, id площадки. Возвращает: строку.
 */
export function venueScopeLabel(def, v) {
  if (def.venue_role !== 'TransferSource') return `площадка ${v}`;
  const kind = store.venueKinds.find((x) => x.id === v)?.kind;
  const cex = store.venueKinds.filter((x) => x.kind === 'Cex').map((x) => x.id);
  if (kind === 'Cex') return `перевод ${v} → кошелёк`;
  if (kind === 'Dex') return `перевод кошелёк (${v}) → ${cex.length ? cex.join(', ') : 'биржа'}`;
  return `перевод с площадки ${v}`;
}
