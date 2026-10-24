// Файл: ui/src/lib/dex.svelte.js
// Что делает: общее хранилище списков адаптера DEX по каждой площадке из GET /api/venues:
//   DEX и виды пулов (GET /api/venues/{id}/dex-families), каналы отправки (GET /api/channels/{id}) и справочник
//   пулов от индексатора (GET /api/venues/{id}/pool-directory, решение 58).
//   Это id областей настроек «DEX» и «канал» — интерфейс не выдумывает их, а берёт у адаптера.
//   Ответ 400 — «не применимо» (биржа): площадка просто не DEX. Иначе ошибка показывается как есть.
//   Перечитывается при старте, при «lagged» (переподключение), по живому сообщению «send_channel» и по записи
//   журнала вида dex.index (справочник обновился) — без таймеров и опроса.
// Кто вызывает: App.svelte (start), экраны «Площадки», «Настройки», «Каналы отправки», components/SettingCard.svelte.
// От чего зависит: lib/api.js, lib/live.svelte.js, lib/settings.svelte.js (список площадок).
// Настройки: нет.

import { get, seg } from './api.js';
import { onLive } from './live.svelte.js';
import { store } from './settings.svelte.js';

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

/** Что делает: первая загрузка и подписка на живой поток. Вызывается один раз из App.svelte. */
export function start() {
  // Список площадок приходит из settings.svelte.js; как только он изменился — перечитать.
  $effect.root(() => {
    $effect(() => {
      store.venues.length;
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
 * Какие id области «DEX» подходят настройке. Чей это id — из схемы бэкенда (SettingDef.dex_id:
 * «Dex» — имя DEX, «Family» — вид пула; src/settings/schema.rs).
 * Принимает: описание настройки. Возвращает: [{id, title}] от адаптеров DEX.
 */
export function dexIdsFor(def) {
  const { dexes, families } = dexScopeIds();
  if (isDexLevel(def)) return dexes.map((d) => ({ id: d.id, title: `DEX ${d.id}` }));
  return families.map((f) => ({ id: f.family, title: f.title }));
}

/** Является ли настройка настройкой уровня DEX (а не вида пула). Принимает: описание. Возвращает: bool. */
export const isDexLevel = (def) => def?.dex_id === 'Dex';
