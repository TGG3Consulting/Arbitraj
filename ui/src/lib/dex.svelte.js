// Файл: ui/src/lib/dex.svelte.js
// Что делает: общее хранилище списков адаптера DEX по каждой площадке из GET /api/venues:
//   DEX и виды пулов (GET /api/venues/{id}/dex-families), каналы отправки (GET /api/channels/{id}) и справочник
//   пулов от индексатора (GET /api/venues/{id}/pool-directory, решение 58).
//   Это id областей настроек «DEX» и «канал» — интерфейс берёт их у адаптера. Решение 128: адаптер сообщает DEX с
//   видом, DEX из справочника пулов и id с уже заданными настройками; DEX, которой нет нигде (её пулы находит только
//   поиск по фабрике, оракул или события Initialize), человек вводит сам — правило id: checkDexId.
//   Ответ 400 — «не применимо» (биржа): площадка просто не DEX. Иначе ошибка показывается как есть.
//   Перечитывается при старте, при «lagged» (переподключение), по живому сообщению «send_channel» и по записи
//   журнала вида dex.index (справочник обновился) — без таймеров и опроса.
// Кто вызывает: App.svelte (start), экраны «Площадки», «Настройки», «Каналы отправки», components/SettingCard.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/settings.svelte.js (список площадок).
// Настройки: нет.

import { get, seg } from './api.js';
import { onLive } from './live.svelte.js';
import { store, valueKey } from './settings.svelte.js';

/**
 * byVenue: id → {families, familiesError, channels, channelsError, directory, directoryError};
 * families/channels/directory = null — нет данных
 * (или «не применимо», тогда ошибка пустая и notDex = true); loaded — ответы получены хоть раз.
 */
export const dex = $state({ byVenue: {}, loaded: false });

/**
 * Что делает: один запрос списка; «не применимо» (400) — не ошибка.
 * Принимает: путь. Возвращает: {list, error, notApplicable}.
 */
async function fetchList(path) {
  try {
    return { list: await get(path), error: '', notApplicable: false };
  } catch (e) {
    return { list: null, error: e.status === 400 ? '' : e.message, notApplicable: e.status === 400 };
  }
}

/** Что делает: перечитывает списки по всем площадкам. Ошибки — в byVenue[id].*Error. */
export async function reloadDex() {
  const ids = [...store.venues];
  const out = {};
  await Promise.all(
    ids.map(async (id) => {
      // Решение 109: биржу (вид Cex из GET /api/venues/kinds) не спрашиваем — у неё этих списков нет (ответ 400 в
      // консоли). Вид ещё не известен — спрашиваем, как раньше.
      if (store.venueKinds.find((x) => x.id === id)?.kind === 'Cex') {
        out[id] = emptyNotDex();
        return;
      }
      const [f, c, d] = await Promise.all([
        fetchList(`/api/venues/${seg(id)}/dex-families`),
        fetchList(`/api/channels/${seg(id)}`),
        fetchList(`/api/venues/${seg(id)}/pool-directory`),
      ]);
      out[id] = {
        families: f.list,
        familiesError: f.error,
        channels: c.list,
        channelsError: c.error,
        directory: d.list,
        directoryError: d.error,
        notDex: f.notApplicable && c.notApplicable,
      };
    }),
  );
  dex.byVenue = out;
  dex.loaded = true;
}

/** Запись «не DEX» без запросов. */
function emptyNotDex() {
  return {
    families: null,
    familiesError: '',
    channels: null,
    channelsError: '',
    directory: null,
    directoryError: '',
    notDex: true,
  };
}

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  // Список площадок и их вид приходят из settings.svelte.js. Решение 109: перечитывать, только когда список или вид
  // площадок действительно изменился — settings.svelte.js переприсваивает их на каждом перечитывании настроек, и
  // справочник пулов (тысячи записей) грузился заново каждый раз.
  let seen = '';
  $effect.root(() => {
    $effect(() => {
      const key = JSON.stringify([store.venues, store.venueKinds]);
      if (key === seen) return;
      seen = key;
      reloadDex();
    });
  });
  onLive((msg) => {
    if (msg.type === 'send_channel') reloadDex();
    if (msg.type === 'journal' && msg.data?.kind === 'dex.index') reloadDex();
  });
}

/**
 * Все id области «DEX» от адаптеров.
 * Возвращает: {dexes: [{id, venue}], families: [DexFamily & {venue}]} — DEX без повторов.
 */
export function dexScopeIds() {
  const dexes = new Map();
  const families = [];
  for (const [venue, v] of Object.entries(dex.byVenue)) {
    for (const f of v.families ?? []) {
      if (!dexes.has(f.dex)) dexes.set(f.dex, { id: f.dex, venue });
      families.push({ ...f, venue });
    }
  }
  return { dexes: [...dexes.values()], families };
}

/**
 * Пулы монеты из справочников всех DEX (решение 58).
 * Принимает: адрес контракта монеты. Возвращает: [{venue, pool, attribution}] — attribution: {text, url} | null.
 */
export function directoryPools(addr) {
  const a = (addr ?? '').toLowerCase();
  const out = [];
  for (const [venue, v] of Object.entries(dex.byVenue)) {
    const d = v.directory;
    if (!d) continue;
    const attribution = d.attribution_text ? { text: d.attribution_text, url: d.attribution_url } : null;
    for (const pool of d.pools) if (pool.token.toLowerCase() === a) out.push({ venue, pool, attribution });
  }
  return out;
}

/** Все каналы от адаптеров. Возвращает: [{venue, info, stats}]. */
export function allChannels() {
  return Object.entries(dex.byVenue).flatMap(([venue, v]) => (v.channels ?? []).map((c) => ({ venue, ...c })));
}

/** Почему списков нет (для «нет данных»). Возвращает: строку причин по площадкам или ''. */
export function whyNoDex() {
  const reasons = Object.entries(dex.byVenue)
    .map(([id, v]) => [id, v.familiesError || v.channelsError])
    .filter(([, r]) => r)
    .map(([id, r]) => `${id}: ${r}`);
  return reasons.join('; ');
}

/**
 * Какие id области «DEX» подходят настройке: DEX из таблицы сопоставления (dex.kind задан, решение 58; id — id DEX
 * у индексатора). Новую DEX подключают в таблице на «Площадках». Принимает: описание (уровень DEX — dex_id «Dex»).
 * Возвращает: [{id, title}] от адаптеров DEX.
 */
export function dexIdsFor(def) {
  const { dexes, families } = dexScopeIds();
  if (isDexLevel(def)) return dexes.map((d) => ({ id: d.id, title: `DEX ${d.id}` }));
  return families.map((f) => ({ id: f.family, title: f.title }));
}

/** Является ли настройка настройкой уровня DEX (а не вида пула). Принимает: описание. Возвращает: bool. */
export const isDexLevel = (def) => def?.dex_id === 'Dex';

/**
 * Что делает: проверка id DEX, который вводят руками (решение 128). Правило то же, что у сервера
 * (settings/scope.rs check_dex_id и check_new_dex_id_len): непустой, только строчная латиница, цифры, «-» и «_»;
 * не длиннее dex.id_max_len (вся система; не задана — без предела). Сервер проверяет то же при сохранении.
 * Принимает: id (уже без пробелов по краям). Возвращает: '' — подходит, иначе причину текстом.
 */
export function checkDexId(id) {
  if (!id) return 'Не указан id DEX';
  const bad = [...id].find((c) => !/[a-z0-9_-]/.test(c));
  if (bad !== undefined) return `Недопустимый символ «${bad}» — только строчная латиница, цифры, «-» и «_»`;
  const max = store.values[valueKey('dex.id_max_len', { kind: 'Global' })];
  if (typeof max === 'number' && [...id].length > max) return `Длиннее ${max} символов (dex.id_max_len)`;
  return '';
}
