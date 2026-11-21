// Файл: ui/src/lib/coinsLive.svelte.js
// Что делает: данные экрана «Монеты» (t84, Тигран 10.11): полный список GET /api/coins и обновление ОДНОЙ строки по
//   живому сообщению монеты — GET /api/coins/{address} (api/coins.rs `one`: та же строка ядра, что в полном списке).
//   Раньше экран перечитывал весь список (~14 МБ) на каждое coin_check / pool_check / coin_block — во время цикла
//   проверки налогов запрос за запросом.
//   - coin_check, pool_check, coin_block → строка монеты data.asset.bsc_contract; pool_check — ещё строки, в пулах
//     которых этот пул (площадка + пул). Строка заменяется на месте: порядок списка — порядок сервера, фильтры и
//     страницы экрана считаются заново из data.
//   - Полный перечит: при открытии, `coins` (справочник пересобран, core/rebuild.rs), «lagged» ядра и переподключение
//     (lib/live.svelte.js шлёт lagged без источника), монета без адреса, строки нет в списке, 404 / сбой строки.
//   - Один запрос в полёте на монету (lib/flight.js: сообщения во время запроса — один повтор после него). Пока идёт
//     полный перечит, строки не запрашиваются: идущие отменяются, монеты копятся и запрашиваются после него —
//     ответ строки, начатый до полного перечита, не перетрёт более свежий полный список. Уход со страницы — отмена.
// Кто вызывает: pages/Coins.svelte (в блоке <script>).
// От чего зависит: lib/api.js (get, seg), lib/live.svelte.js, lib/flight.js, lib/coinFilter.js (coinRowKey).
// Настройки: нет.

import { get, seg } from './api.js';
import { onLive, lagFor } from './live.svelte.js';
import { flight } from './flight.js';
import { coinRowKey } from './coinFilter.js';

/** Сообщения по одной монете — обновить её строку. */
const ROW_TYPES = ['coin_check', 'pool_check', 'coin_block'];

/**
 * Что делает: создаёт состояние экрана и подписку (живёт, пока жив компонент).
 * Принимает: ничего. Возвращает: $state {data, noData, error, loading} — data = null, пока данных нет.
 */
export function coinsLive() {
  const st = $state({ data: null, noData: '', error: '', loading: true });
  /** Полёты строк: ключ строки (адрес в нижнем регистре) → run (lib/flight.js). */
  const rows = new Map();
  /** Монеты, чьи строки нужно запросить после полного перечита: ключ → адрес. */
  const dirty = new Map();
  let fullBusy = false;
  let alive = true;

  /** Полный перечит (signal — отмена). Ошибки — в noData (503) или error; отмена — ничего. */
  async function loadAll(signal) {
    try {
      st.data = await get('/api/coins', signal);
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
  const full = flight(loadAll);

  /** Полный перечит закончен: накопленные строки — запросить (данных нет — их покроет следующий перечит). */
  function fullDone() {
    fullBusy = false;
    if (!alive) return;
    const list = [...dirty.values()];
    dirty.clear();
    if (st.data) for (const a of list) row(a);
  }

  /** Полный перечит: идущие запросы строк отменяются (их монеты — после перечита). */
  function reloadAll() {
    fullBusy = true;
    for (const [k, run] of rows) {
      run.cancel();
      dirty.set(k, run.address);
    }
    rows.clear();
    full().then(fullDone, fullDone);
  }

  /** Строка монеты address: один полёт на монету. Ошибки: 404 / сбой — полный перечит; отмена — ничего. */
  function row(address) {
    const k = address.toLowerCase();
    if (fullBusy) {
      dirty.set(k, address);
      return;
    }
    let run = rows.get(k);
    if (!run) {
      run = flight(async (signal) => {
        let c;
        try {
          c = await get(`/api/coins/${seg(address)}`, signal);
        } catch (e) {
          if (!e.aborted) fallback();
          return;
        }
        const i = Array.isArray(st.data) ? st.data.findIndex((x) => coinRowKey(x) === k) : -1;
        // Строки нет в списке (монета появилась без пересборки) — место знает только полный список.
        if (i < 0) fallback();
        else st.data[i] = c;
      });
      /** Полный перечит вместо строки: сама строка после него не запрашивается (полный список её уже несёт). */
      const fallback = () => {
        if (rows.get(k) === run) rows.delete(k);
        run.cancel();
        reloadAll();
      };
      run.address = address;
      rows.set(k, run);
    }
    const my = run;
    my().then(() => {
      if (rows.get(k) === my) rows.delete(k);
    });
  }

  /** Живое сообщение по монете → её строка (и строки с этим пулом); без адреса — полный перечит. */
  function onRow(msg) {
    const a = msg.data?.asset?.bsc_contract;
    if (!a || !Array.isArray(st.data)) return reloadAll();
    row(a);
    if (msg.type !== 'pool_check') return;
    const pool = JSON.stringify(msg.data.pool);
    const venue = JSON.stringify(msg.data.venue);
    for (const c of st.data) {
      const has = (c.pools ?? []).some((p) => JSON.stringify(p.venue) === venue && JSON.stringify(p.pool) === pool);
      if (has && c.asset?.bsc_contract) row(c.asset.bsc_contract);
    }
  }

  $effect(() => {
    alive = true;
    reloadAll();
    const off = onLive((msg) => {
      if (lagFor(msg, 'core') || msg.type === 'coins') reloadAll();
      else if (ROW_TYPES.includes(msg.type)) onRow(msg);
    });
    return () => {
      alive = false;
      off();
      full.cancel();
      for (const run of rows.values()) run.cancel();
      rows.clear();
      dirty.clear();
    };
  });
  return st;
}
