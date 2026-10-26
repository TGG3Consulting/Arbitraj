// Файл: tests/recorders/record_ws_logs.js
// Что делает: записывает реальный WS-поток eth_subscribe("logs") публичного узла BSC для решения 79 (вернуть
//   подписку logs как источник событий пулов вместо тяни-квитанции-каждого-блока). Запускается в консоли вкладки
//   браузера, как record_receipts.js / record_depth.js; ключ не нужен (узел публичный). Ничего не торгует, только
//   читает и складывает в window.__rec.
// Кто вызывает: Тигран в браузере (техлид разбирает запись при проектировании 79).
// От чего зависит: ничего (чистый браузерный WebSocket).
// Внимание: только официальные публичные узлы BNB Chain (CLAUDE.md, решение 23). Разбор формата кадров — ПОСЛЕ этой
//   записи, по её содержимому, а не по догадке.
//
// Как запустить:
//   1) открыть любую вкладку (например https://bnbchain.org), F12 → Console;
//   2) вставить весь файл, Enter;
//   3) ждать RUN_SECONDS (по умолчанию 120 с) — в конце в консоли одна строка-итог «WS-RECORD DONE …»;
//   4) выполнить  copy(JSON.stringify(window.__rec))  и прислать техлиду (или сохранить в файл).
//
// Что собирает:
//   subs   — отправленные подписки и ответы узла (id/result или error);
//   frames — каждый пришедший кадр eth_subscription (время, подписка, адрес пула, topic0, блок, logIndex, removed);
//   limits — отдельные пробы пределов: сколько адресов принимает один фильтр logs (SIZES), сколько подписок на одно
//            соединение (SUBS_PROBE), и не пришёл ли removed:true (виден в frames[].removed).

(function () {
  "use strict";

  // ── Настройки (плейсхолдеры можно заменить перед запуском) ──────────────────────────────────────────────
  // Основной узел — из официального списка BNB Chain (geth, поддерживает eth_subscribe по WSS).
  const NODE = "wss://bsc-rpc.publicnode.com";
  // Запасные WSS, если основной не открывается из браузера (CORS/Origin/заголовки). Брать только из офсписка
  // https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ — ниже пример, Тигран проверит/заменит.
  const FALLBACKS = ["wss://bsc.meowrpc.com", "wss://bsc.drpc.org"];
  // Сколько секунд слушать основной поток.
  const RUN_SECONDS = 120;
  // Максимум кадров в записи (чтобы не раздуть JSON): лишние только считаются.
  const MAX_FRAMES = 4000;

  // Адреса наших пулов V2/V3 из записи глубины tests/data/depth_2026-10-25 (контрактные пулы — у них есть адрес,
  // по нему и фильтрует logs; у Infinity CL / Uniswap V4 события шлёт менеджер с PoolId в topics[1] — это отдельная
  // проба, см. ниже; плейсхолдер ADDR техлид при желании заменит/дополнит).
  const ADDR = [
    "0x9154c891ebf6bacf707970c501f58fecfe3a5bac", // ZKP uni_v3
    "0x8d9318c55bd2614cbccba822a19898250299f813", // IOST pcs_v3
    "0xdbea6cbcf71ca3602a9c622f2734485ce80d3f6e", // IOST pcs_v3
    "0x5fef5bcc62164f8fb0d53d58733cc3940ddfc46c", // IOST v2
    "0xfc5fd324365b8feb342aefa376e491b5b17edf63", // IOST pcs_v3
    "0x594eccd06f64536bb278af508bf786c99574538a", // IOST v2
    "0xa135061c34abdeb1cd6ec492d8355318b5c8afe2", // IOST v2
    "0x8b22c20cb60ae915524fb46e1c1b101b50168445", // AAVE pcs_v3
    "0x3373a22cb07cb49651b82cf6f174ef434e4dbaa8", // AAVE uni_v3
    "0x8e3ecc0b261f1a4db62321090575eb299844f077", // AAVE uni_v3
    "0xcaebd546486d3af55e63c0f165812f981c10f476", // AAVE uni_v3
    "0xe98fc882596c307ff26f8c570532ed0e93238e9a", // AAVE pcs_v3
    "0x60f481ace519b6a20e01c2b9152923697bdd7b9f", // AAVE pcs_v3
    "0x28316d69db3547428dee6a0e2053754dba9d42ab", // AAVE v2
    "0xe91d7ddcc2a11e57007951aa65a76c7fac2fcb2b", // AAVE v2
    "0xc31881aa86a2b3ca2ee8e5c8979c5fbcc376426e", // AAVE v2
    "0x2e31285ade0e055036e709fc4f1b2c56c6e8fc0f", // AAVE pcs_v3
  ];
  // Размеры фильтра для пробы «сколько адресов принимает один logs»: где узел начнёт отказывать — там предел дробления
  // ~2850 пулов на пачки.
  const SIZES = [1, 100, 500, 1000, 2000, 5000];
  // Сколько подписок подряд на одно соединение попробовать (счёт успешных до первой ошибки).
  const SUBS_PROBE = 200;

  // ── Режим ───────────────────────────────────────────────────────────────────────────────────────────────
  // "main"     — поток logs по адресам пулов V2/V3 (как первая запись);
  // "managers" — решение 79, CL/V4: события шлёт АДРЕС МЕНЕДЖЕРА, PoolId в topics[1]. Проверяем вариант (а):
  //   (1) сколько PoolId принимает фильтр topics[1] у каждого менеджера (предел дробления CL/V4);
  //   (2) короткий «файрхоз» менеджера БЕЗ фильтра PoolId (вариант (б)) — померить объём, НЕ грузить (30 с, жёсткий
  //       лимит кадров MAX_FRAMES, лишние только считаются), заодно собрать реально встреченные topic0;
  //   (3) сколько соединений на IP держится (CONN_PROBE) — запас под отдельные подписки на каждого менеджера.
  const MODE = "managers";

  // Менеджеры CL/V4 (адреса-эмитенты событий, используются в by_pool_id; сверено с dex.pool_manager и тестами
  // bsc_dex_batch_load/bsc_dex_node_calls). CL — PancakeSwap Infinity CLPoolManager; V4 — Uniswap V4 PoolManager.
  const MANAGERS = [
    { kind: "infinity_cl", manager: "0xa0ffb9c1ce1fe56963b0321b32e7a0302114058b" },
    { kind: "uniswap_v4", manager: "0x28e2ea090877bf75740558f6bfb36a5ffee9e9df" },
  ];
  // Наши PoolId из записи глубины tests/data/depth_2026-10-25 (topics[1]). CL — 13, V4 — 45.
  const POOL_IDS = {
    infinity_cl: [
      "0x841f7ca671b4dd2c3920884cf045e87762ab549506fd2a2ae40f4b26cb81ad6a",
      "0xae19c3aa5fb0d6f37e01251fb16c05bb2b9b146b8b932d431d5b29bbd6df8f2a",
      "0x1ee478fa618b152bb09e4c1a805b83c11a2cc328ab35a114a2be7360811fa572",
    ],
    uniswap_v4: [
      "0xbe42d277f1865d14f5787bff0930099d9f4d3748eedaa2e983aa0bae7c36d670",
      "0x438d67d4899bf77855c9d2f3ab45855fd66b9860b5653fbecfac61b2b707ddc6",
      "0xa2bf5402ad19bd310136dab3e8d281bd9266d523de48eae727d515ef7dbbf53e",
    ],
  };
  // Размеры фильтра topics[1] для менеджеров (где откажет — предел дробления CL/V4).
  const ID_SIZES = [1, 50, 200, 1000, 5000];
  // Короткий файрхоз менеджера без фильтра PoolId — чтобы померить объём варианта (б).
  const FIREHOSE_SECONDS = 30;
  // Сколько соединений открыть и подержать (предел на IP; в офдоках publicnode числа может не быть).
  const CONN_PROBE = 10;
  const CONN_HOLD_SECONDS = 20;

  // ── Запись ──────────────────────────────────────────────────────────────────────────────────────────────
  const rec = {
    about:
      "Решение 79: реальный WS eth_subscribe(logs) публичного узла BSC. Режим в поле mode. main — поток по адресам " +
      "пулов V2/V3. managers — CL/V4: предел PoolId в topics[1] у менеджера (limits.poolids_in_topic1), короткий файрхоз " +
      "менеджера без фильтра PoolId для объёма варианта (б) (manager_firehose), предел соединений на IP (connections). " +
      "Селекторы событий берём из пути квитанций (официальные ABI), не из этой записи. Разбор — после записи.",
    mode: MODE,
    node: NODE,
    startedAt: new Date().toISOString(),
    subs: [],
    frames: [],
    framesDropped: 0,
    limits: [],
    notes: [],
  };
  window.__rec = rec;

  const nowMs = () => Date.now();
  const log = (...a) => console.log("[ws-record]", ...a);

  // Детерминированный псевдо-адрес для набивки больших фильтров (не матчит логи — проверяем ТОЛЬКО приём фильтра).
  function padAddr(i) {
    const h = (i >>> 0).toString(16).padStart(40, "0");
    return "0x" + h;
  }
  function addrList(size) {
    const out = [];
    for (let i = 0; i < size; i++) out.push(ADDR[i % ADDR.length] || padAddr(i));
    // Разнообразим набивку, чтобы не схлопнулось в дубли на стороне узла.
    if (size > ADDR.length) {
      for (let i = ADDR.length; i < size; i++) out[i] = padAddr(i);
    }
    return out;
  }

  function open(url) {
    return new Promise((resolve, reject) => {
      let ws;
      try {
        ws = new WebSocket(url);
      } catch (e) {
        reject(e);
        return;
      }
      const t = setTimeout(() => reject(new Error("таймаут подключения")), 15000);
      ws.onopen = () => {
        clearTimeout(t);
        resolve(ws);
      };
      ws.onerror = (e) => {
        clearTimeout(t);
        reject(e);
      };
    });
  }

  // Одна пачка запрос→ответ на отдельном соединении (для проб пределов).
  function once(url, id, method, params, timeoutMs) {
    return open(url).then(
      (ws) =>
        new Promise((resolve) => {
          const done = (res) => {
            try { ws.close(); } catch (e) {}
            resolve(res);
          };
          const t = setTimeout(() => done({ ok: false, error: "нет ответа за " + timeoutMs + " мс" }), timeoutMs);
          ws.onmessage = (ev) => {
            clearTimeout(t);
            let m;
            try { m = JSON.parse(ev.data); } catch (e) { return done({ ok: false, error: "ответ не JSON" }); }
            if (m.error) return done({ ok: false, error: m.error });
            done({ ok: true, result: m.result });
          };
          ws.onclose = () => { clearTimeout(t); done({ ok: false, error: "соединение закрыто" }); };
          ws.send(JSON.stringify({ jsonrpc: "2.0", id: id, method: method, params: params }));
        }),
      (e) => ({ ok: false, error: "не подключиться: " + (e && e.message ? e.message : String(e)) })
    );
  }

  // Проба «сколько адресов в одном фильтре logs».
  async function probeSizes(url) {
    for (const size of SIZES) {
      const res = await once(url, 9000 + size, "eth_subscribe", ["logs", { address: addrList(size) }], 20000);
      rec.limits.push({ type: "addresses_per_filter", size: size, ok: res.ok, result: res.result, error: res.error });
      log("предел адресов:", size, res.ok ? "ok " + res.result : "отказ " + JSON.stringify(res.error));
    }
  }

  // Проба «сколько подписок на одно соединение» (по одной подписке на адрес, считаем успешные до первой ошибки).
  function probeSubsPerConn(url) {
    return open(url).then(
      (ws) =>
        new Promise((resolve) => {
          let okCount = 0;
          let firstErr = null;
          const total = SUBS_PROBE;
          const t = setTimeout(finish, 25000);
          function finish() {
            clearTimeout(t);
            try { ws.close(); } catch (e) {}
            rec.limits.push({ type: "subs_per_conn", sent: total, ok_count: okCount, error: firstErr });
            log("подписок на соединение: успешно", okCount, "из", total, firstErr ? "первая ошибка " + JSON.stringify(firstErr) : "");
            resolve();
          }
          ws.onmessage = (ev) => {
            let m;
            try { m = JSON.parse(ev.data); } catch (e) { return; }
            if (m.id && m.id >= 20000) {
              if (m.error) { if (!firstErr) firstErr = m.error; }
              else okCount++;
              if (okCount + (firstErr ? 1 : 0) >= total || m.error) {
                // дослушаем ещё немного ответов, затем завершим
              }
            }
          };
          ws.onclose = finish;
          for (let i = 0; i < total; i++) {
            ws.send(JSON.stringify({ jsonrpc: "2.0", id: 20000 + i, method: "eth_subscribe", params: ["logs", { address: ADDR[i % ADDR.length] }] }));
          }
          setTimeout(finish, 8000);
        }),
      (e) => { rec.limits.push({ type: "subs_per_conn", error: "не подключиться: " + String(e) }); }
    );
  }

  // Детерминированный псевдо-PoolId (32 байта) для набивки больших topics[1] — проверяем ТОЛЬКО приём фильтра.
  function padId(i) {
    const h = (i >>> 0).toString(16).padStart(64, "0");
    return "0x" + h;
  }
  function idList(ids, size) {
    const out = [];
    for (let i = 0; i < size; i++) out.push(i < ids.length ? ids[i] : padId(i));
    return out;
  }

  // Открывает первый доступный узел (основной или запасной). Возвращает {ws,url} или null.
  async function openAny() {
    for (const cand of [NODE].concat(FALLBACKS)) {
      try {
        const ws = await open(cand);
        return { ws, url: cand };
      } catch (e) {
        rec.notes.push("узел " + cand + " не открылся: " + (e && e.message ? e.message : String(e)));
        log("не открылся", cand);
      }
    }
    return null;
  }

  // ── Режим managers (решение 79, CL/V4) ─────────────────────────────────────────────────────────────────
  // Проба «сколько PoolId принимает topics[1] у менеджера».
  async function probePoolIds(url, mgr) {
    const ids = POOL_IDS[mgr.kind] || [];
    for (const size of ID_SIZES) {
      const filter = { address: mgr.manager, topics: [null, idList(ids, size)] };
      const res = await once(url, 30000 + size, "eth_subscribe", ["logs", filter], 20000);
      rec.limits.push({
        type: "poolids_in_topic1",
        kind: mgr.kind,
        manager: mgr.manager,
        size: size,
        ok: res.ok,
        result: res.result,
        error: res.error,
      });
      log("PoolId в topics[1]", mgr.kind, size, res.ok ? "ok" : "отказ " + JSON.stringify(res.error));
    }
  }

  // Короткий файрхоз менеджера БЕЗ фильтра PoolId: объём варианта (б). Жёсткий лимит кадров, лишние считаются;
  // собираем реально встреченные topic0 (эмпирически, не как источник селекторов — те из ABI квитанций).
  function probeFirehose(url, mgr) {
    return open(url).then(
      (ws) =>
        new Promise((resolve) => {
          let frames = 0;
          let dropped = 0;
          const topics = {};
          const start = nowMs();
          const t = setTimeout(finish, FIREHOSE_SECONDS * 1000 + 3000);
          function finish() {
            clearTimeout(t);
            try { ws.close(); } catch (e) {}
            rec.limits.push({
              type: "manager_firehose",
              kind: mgr.kind,
              manager: mgr.manager,
              seconds: Math.round((nowMs() - start) / 1000),
              frames: frames,
              dropped: dropped,
              per_sec: +(frames / FIREHOSE_SECONDS).toFixed(1),
              topic0_seen: topics,
            });
            log("файрхоз менеджера", mgr.kind, "кадров", frames, "(+" + dropped + ")", "~" + (frames / FIREHOSE_SECONDS).toFixed(1) + "/с");
            resolve();
          }
          ws.onmessage = (ev) => {
            let m;
            try { m = JSON.parse(ev.data); } catch (e) { return; }
            if (m.method === "eth_subscription" && m.params && m.params.result) {
              const r = m.params.result;
              const t0 = r.topics && r.topics[0];
              if (t0) topics[t0] = (topics[t0] || 0) + 1;
              if (frames < MAX_FRAMES) {
                frames++;
              } else {
                dropped++;
              }
            }
          };
          ws.onclose = finish;
          // Без topics[1]: ВСЕ события менеджера (все пулы DEX) — верхняя оценка объёма варианта (б).
          ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "eth_subscribe", params: ["logs", { address: mgr.manager }] }));
          setTimeout(finish, FIREHOSE_SECONDS * 1000);
        }),
      (e) => { rec.limits.push({ type: "manager_firehose", kind: mgr.kind, error: "не подключиться: " + String(e) }); }
    );
  }

  // Проба «сколько соединений на IP держится»: открыть CONN_PROBE соединений с 1 подпиской, подержать.
  async function probeConnections(url) {
    const conns = [];
    let ok = 0;
    const errors = [];
    for (let i = 0; i < CONN_PROBE; i++) {
      try {
        const ws = await open(url);
        ws.send(JSON.stringify({ jsonrpc: "2.0", id: 40000 + i, method: "eth_subscribe", params: ["logs", { address: ADDR[0] }] }));
        conns.push(ws);
        ok++;
      } catch (e) {
        errors.push("соединение " + i + ": " + (e && e.message ? e.message : String(e)));
      }
    }
    await new Promise((r) => setTimeout(r, CONN_HOLD_SECONDS * 1000));
    const alive = conns.filter((w) => w.readyState === 1).length;
    for (const w of conns) { try { w.close(); } catch (e) {} }
    rec.limits.push({ type: "connections", opened: ok, alive_after_hold: alive, errors: errors });
    log("соединений открыто", ok, "из", CONN_PROBE, "живы после паузы", alive);
  }

  async function managers() {
    const picked = await openAny();
    if (!picked) {
      rec.notes.push("ни один узел не открылся — возможно, нужен другой WSS из офсписка BNB");
      log("WS-RECORD DONE: ни один узел не открылся");
      return;
    }
    try { picked.ws.close(); } catch (e) {}
    rec.node = picked.url;
    // Пределы PoolId в topics[1] — по каждому менеджеру, по очереди (не грузим узел параллелью).
    for (const mgr of MANAGERS) await probePoolIds(picked.url, mgr);
    // Файрхоз каждого менеджера — коротко, по очереди.
    for (const mgr of MANAGERS) await probeFirehose(picked.url, mgr);
    // Предел соединений на IP.
    await probeConnections(picked.url);
    rec.endedAt = new Date().toISOString();
    console.log(
      "WS-RECORD DONE (managers): узел " + rec.node +
        ", проб пределов " + rec.limits.length +
        ". Теперь: copy(JSON.stringify(window.__rec))"
    );
  }

  // Основной поток: подписка logs по адресам ADDR, запись всех кадров.
  async function main() {
    const picked = await openAny();
    if (!picked) {
      rec.notes.push("ни один узел не открылся из браузера — возможно, нужен другой WSS из офсписка BNB");
      log("WS-RECORD DONE: ни один узел не открылся");
      return;
    }
    const ws = picked.ws;
    const url = picked.url;
    rec.node = url;
    const subId = new Map(); // result → наш id
    ws.onmessage = (ev) => {
      let m;
      try { m = JSON.parse(ev.data); } catch (e) { return; }
      if (m.id === 1) {
        rec.subs.push({ id: 1, params: ["logs", { address: ADDR }], ok: !m.error, result: m.result, error: m.error });
        if (m.result) subId.set(m.result, 1);
        log("основная подписка logs:", m.error ? "отказ " + JSON.stringify(m.error) : "ok " + m.result);
        return;
      }
      if (m.method === "eth_subscription" && m.params && m.params.result) {
        const r = m.params.result;
        if (rec.frames.length < MAX_FRAMES) {
          rec.frames.push({
            t: nowMs(),
            sub: m.params.subscription,
            address: r.address,
            topic0: r.topics && r.topics[0],
            topics: r.topics,
            blockNumber: r.blockNumber,
            logIndex: r.logIndex,
            txHash: r.transactionHash,
            removed: r.removed === true,
          });
        } else {
          rec.framesDropped++;
        }
      }
    };
    // Подписка по адресам без topics — ловим ВСЕ события этих пулов (Sync/Swap/Mint/Burn и пр.); какие именно topic0
    // приходят, видно в frames — по ним и спроектируем фильтр по топикам (без догадки, решение 23).
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "eth_subscribe", params: ["logs", { address: ADDR }] }));

    // Пробы пределов — на отдельных соединениях, параллельно потоку.
    probeSizes(url);
    probeSubsPerConn(url);

    setTimeout(() => {
      try { ws.close(); } catch (e) {}
      rec.endedAt = new Date().toISOString();
      const t0 = rec.frames.length ? rec.frames[0].removed : null;
      console.log(
        "WS-RECORD DONE: узел " + rec.node +
          ", кадров " + rec.frames.length + (rec.framesDropped ? " (+" + rec.framesDropped + " сверх лимита)" : "") +
          ", подписок " + rec.subs.length +
          ", проб пределов " + rec.limits.length +
          ", removed встречался: " + rec.frames.some((f) => f.removed) +
          ". Теперь: copy(JSON.stringify(window.__rec))"
      );
    }, RUN_SECONDS * 1000);
  }

  if (MODE === "managers") managers();
  else main();
})();
