// Файл: ui/src/lib/settings.svelte.js
// Что делает: общее для экранов хранилище — схема настроек и событий (GET /api/settings/schema),
//   заданные значения (GET /api/settings), список площадок (GET /api/venues) и их вид (GET /api/venues/kinds).
//   Значения обновляются на лету из живого потока ({"type":"settings"}), при «lagged» — перечитываются.
//   Значений по умолчанию нет: не задано — значит в values нет записи.
// Кто вызывает: App.svelte (start), экраны «Настройки», «Уведомления», «Площадки», components/SettingEditor.svelte,
//   components/SettingCard.svelte и pages/Venues.svelte (shownAtVenue, решение 283).
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/flight.js (один запрос в полёте).
// Настройки: читает и показывает любые.

import { get, put } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';

/** schema: {settings, events} или null; values: ключ области → значение; venues: id площадок;
 *  venueKinds: [{id, kind: 'Cex' | 'Dex'}]. */
export const store = $state({ schema: null, values: {}, venues: [], venueKinds: [], error: '' });

/**
 * Ключ значения в store.values.
 * Принимает: имя настройки, область {kind, id?}. Возвращает: строку «имя|вид|id».
 */
export const valueKey = (name, scope) => `${name}|${scope.kind}|${scope.id ?? ''}`;

/** Что делает: перечитывает всё по REST (один запрос в полёте — lib/flight.js). Ошибка — в store.error. */
export const reload = flight(async () => {
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
});

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  reload();
  onLive((msg) => {
    if (lagFor(msg, 'settings')) reload();
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
export async function setValue(name, scope, value, compare) {
  const body = { name, scope: scope.kind === 'Global' ? { kind: 'Global' } : scope, value };
  if (compare !== undefined) body.compare = {value:compare};
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
 * Решение 283: показывать ли настройку области «площадка» у этой площадки (только показ; чтение и запись не меняются).
 * В схеме поле venue_for: «Dex» — значение читается только у площадки DEX, у биржи (вид «Cex» из GET /api/venues/kinds)
 * строка не показывается; вид площадки неизвестен или venue_for другое — показывается, как раньше.
 * Принимает: описание настройки, id площадки. Возвращает: bool.
 */
export function shownAtVenue(def, v) {
  if (def.venue_for !== 'Dex') return true;
  return store.venueKinds.find((x) => x.id === v)?.kind !== 'Cex';
}

/**
 * Подпись области «площадка» у настройки. Решение 118 (Я-4, Я-23): время в пути = отправка у источника + зачисление у
 * получателя, поэтому подпись — без пары «откуда → куда» (с двумя биржами у источника несколько получателей):
 * venue_role «TransferSource» (решение 51) — «отправка с площадки (до появления в сети)»; «TransferTarget» —
 * «зачисление на площадке после появления в сети». У DEX площадка — свой кошелёк.
 * Принимает: описание настройки, id площадки. Возвращает: строку.
 */
export function venueScopeLabel(def, v) {
  const kind = store.venueKinds.find((x) => x.id === v)?.kind;
  const name = kind === 'Dex' ? `кошелька (${v})` : v;
  if (def.venue_role === 'TransferSource') return `отправка с ${name} (до появления в сети)`;
  if (def.venue_role === 'TransferTarget') {
    return `зачисление на ${kind === 'Dex' ? `кошелёк (${v})` : v} после появления в сети`;
  }
  return `площадка ${v}`;
}
