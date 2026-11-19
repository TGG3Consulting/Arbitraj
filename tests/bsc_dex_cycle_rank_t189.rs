//! Файл: tests/bsc_dex_cycle_rank_t189.rs
//! Что делает: решение 404 (t189) — отбор N ЛУЧШИХ циклов DEX ↔ DEX по курсу круга (quote/cycle_rank.rs) вместо первых
//! найденных, на РЕАЛЬНОЙ записи маршрута CAKE 2026-10-29 (tests/data/routes_2026-10-29, та же, что у
//! tests/bsc_dex_cycle.rs: шесть пулов на блоке 130521896 — CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT V2, CAKE-BTCB, BTCB-WBNB).
//! Проверяется без моков: (1) из M циклов базовой USDT длиной до 4 (route_paths::enumerate_cycles — все) отбор N берёт
//! ровно N с наибольшим курсом круга (произведение курсов рёбер cycle_rate::edge_rate; стороны всех пулов глубокие — впереди), а не первые по перебору; курсы
//! отобранных — по убыванию; просмотрено ровно M; (2) детерминированность: два графа из одних пулов, положенных в реестр
//! в разном порядке, и повторный вызов дают один и тот же отбор; отбор N — начало отбора M; (3) предел просмотра
//! dex_dex.rank_max_cycles останавливает перебор с отметкой; (4) причина отказа пишется: пул, не допущенный к котировке
//! (нет сверки с квотером и пробы газа — проверок на этом окружении нет), даёт текст причины quote/cycle.rs
//! legs_for_cycle — он попадает в итог прохода (quote/cycle_tally.rs, строка и данные), а неисполнимый сигнал — в
//! строку отбора ядра (core/cycle_scan_log.rs cands_line) с примером причины.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote}, arb::core::cycle_scan_log, abi_v2v3; настоящая SQLite во
//! временной папке.

use std::collections::{BTreeMap, HashMap};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::abi_v2v3::{
    IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool,
};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::cycle;
use arb::venues::bsc_dex::quote::cycle_rank;
use arb::venues::bsc_dex::quote::cycle_rate::{self, Rates};
use arb::venues::bsc_dex::quote::cycle_tally::{Lost, Tally};
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::route_paths::{self, RoutePath};
use arb::venues::bsc_dex::quote::select::Usable;

// Узлы графа: порядок сортировки — как у реальных адресов BSC (CAKE < USDT < BTCB < WBNB), чтобы token0 < token1 у V3
// совпадало с записанным sqrtPriceX96. Сами адреса условные (запись их не содержит).
const CAKE: Address = address!("0x0e00000000000000000000000000000000000001");
const USDT: Address = address!("0x5500000000000000000000000000000000000002");
const BTCB: Address = address!("0x7100000000000000000000000000000000000003");
const WBNB: Address = address!("0xbb00000000000000000000000000000000000004");

/// Записанные ответы: имя вызова → байты результата.
fn load() -> HashMap<String, Vec<u8>> {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/routes_2026-10-29/responses.json").unwrap(),
    )
    .unwrap();
    let mut got = HashMap::new();
    for r in v["results"].as_array().unwrap() {
        if let Some(s) = r["response"]["result"].as_str() {
            got.insert(
                r["name"].as_str().unwrap().to_string(),
                alloy_primitives::hex::decode(s).unwrap(),
            );
        }
    }
    got
}

fn base_entry(
    kind: &str,
    fee: Option<u32>,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let f = families::make(kind, kind).unwrap();
    let (t0, t1) = if coin < counter {
        (coin, counter)
    } else {
        (counter, coin)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: fee,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin,
        usdt: counter,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

/// Пул V3 из записи (slot0 + liquidity + слова TickLens) как запись реестра. pancake — ABI PancakeSwap (slot0 шире).
#[allow(clippy::too_many_arguments)]
fn v3(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    pancake: bool,
    spacing: i32,
    fee: u32,
    words: (i16, i16),
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let (sqrt, tick) = if pancake {
        let s =
            IPancakeV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    } else {
        let s =
            IUniswapV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    };
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&got[&format!("{tag} liquidity")]).unwrap();
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(
            &got[&format!("{tag} TickLens word {k}")],
        )
        .unwrap()
        {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    let kind = if pancake {
        "pancakeswap_v3"
    } else {
        "uniswap_v3"
    };
    let mut e = base_entry(kind, Some(fee), addr, coin, counter);
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: sqrt,
        liquidity,
        tick,
    };
    e.ticks = Some(TickMap {
        spacing,
        ticks: ticks.into(),
        words,
    });
    e
}

/// Пул V2 из записи (getReserves). Комиссия 0,25% (fee_pips 2500).
fn v2(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let rs =
        IV2Pair::getReservesCall::abi_decode_returns(&got[&format!("{tag} getReserves")]).unwrap();
    let mut e = base_entry("pancakeswap_v2", Some(2500), addr, coin, counter);
    e.pool.state = PoolState::V2 {
        reserve0: U256::from(rs.reserve0),
        reserve1: U256::from(rs.reserve1),
    };
    e
}

async fn ctx() -> Ctx {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let d = std::env::temp_dir().join(format!("arb-cycle-rank-{uniq}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        chains: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
    ] {
        c.settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пулы записи (как в tests/bsc_dex_cycle.rs graph_and_ctx). Ошибок нет.
fn pools() -> Vec<Entry> {
    let got = load();
    vec![
        v3(
            &got,
            "CAKE-USDT pcs_v3",
            true,
            50,
            2500,
            (-3, 3),
            address!("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-USDT uni_v3",
            false,
            60,
            3000,
            (-3, 3),
            address!("0xfe4fe5b4575c036ac6d5cccfe13660020270e27a"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-WBNB pcs_v3",
            true,
            50,
            2500,
            (-8, -2),
            address!("0x133b3d95bad5405d14d53473671200e9342896bf"),
            CAKE,
            WBNB,
        ),
        v2(
            &got,
            "WBNB-USDT v2",
            address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae"),
            WBNB,
            USDT,
        ),
        v3(
            &got,
            "CAKE-BTCB uni_v3",
            false,
            200,
            10000,
            (-3, 3),
            address!("0x8e8410f4878079fc03a287a7ded4a65afb87cc06"),
            BTCB,
            CAKE,
        ),
        v3(
            &got,
            "BTCB-WBNB pcs_v3",
            true,
            10,
            500,
            (15, 21),
            address!("0x6bbc40579ad1bbd243895ca0acb086bb6300d636"),
            BTCB,
            WBNB,
        ),
    ]
}

/// Граф из записи; reverse — пулы кладутся в реестр в обратном порядке (другой порядок рёбер). Ошибок нет.
async fn graph_and_parts(reverse: bool) -> (graph::Graph, Parts) {
    let p = Parts::new(ctx().await);
    p.tokens.set(&[Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(USDT),
    }]);
    let mut list = pools();
    if reverse {
        list.reverse();
    }
    for e in list {
        p.registry.put(e);
    }
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    (g, p)
}

/// Ключ цикла: пул и токен входа каждого шага.
fn key(path: &RoutePath) -> Key {
    path.steps
        .iter()
        .map(|s| (s.edge.entry.pool.reference.clone(), s.token_in))
        .collect()
}

/// Курс круга цикла — произведение курсов рёбер (тот же показатель, что у отбора). Ошибок нет: нет курса — паника.
fn rate_of(p: &Parts, path: &RoutePath) -> Decimal {
    path.steps.iter().fold(Decimal::ONE, |acc, s| {
        acc * cycle_rate::edge_rate(&p.ctx, &s.edge, s.token_in).expect("у ребра записи есть курс")
    })
}

/// Ключ цикла: пул и токен входа каждого шага.
type Key = Vec<(PoolRef, Address)>;

/// Цикл в ожидаемом порядке отбора: все стороны глубокие, курс круга, ключ.
type Scored = (bool, Decimal, Key);

/// Все стороны пулов цикла глубокие по книге глубины (тот же признак, что у отбора: вход — встречный токен пула →
/// покупка, вход — монета → продажа). Ошибок нет.
fn deep_of(p: &Parts, path: &RoutePath) -> bool {
    path.steps.iter().all(|s| {
        p.registry
            .depth()
            .last(&s.edge.entry.pool.reference)
            .is_some_and(|d| {
                if s.token_in == s.edge.entry.coin {
                    d.deep_sell
                } else {
                    d.deep_buy
                }
            })
    })
}

/// Отбор N лучших по базовой USDT, длина до 4. Ошибок нет.
fn top(g: &graph::Graph, p: &Parts, n: usize, cap: Option<u64>) -> cycle_rank::Ranked {
    let mut rates = Rates::new(g, p.registry.depth());
    cycle_rank::top(&p.ctx, g, &mut rates, USDT, 4, n, cap)
}

#[tokio::test]
async fn picks_best_by_cycle_rate_not_first() {
    let (g, p) = graph_and_parts(false).await;
    let all = route_paths::enumerate_cycles(&g, USDT, 4, 10_000);
    let m = all.len();
    assert!(m >= 4, "на записи есть несколько циклов базовой USDT: {m}");
    // Порядок отбора: все стороны глубокие — впереди, затем курс круга.
    let mut scored: Vec<Scored> = all
        .iter()
        .map(|c| (deep_of(&p, c), rate_of(&p, c), key(c)))
        .collect();
    scored.sort_by_key(|x| std::cmp::Reverse((x.0, x.1)));
    let n = 2;
    let r = top(&g, &p, n, None);
    assert_eq!(r.seen as usize, m, "просмотрены все циклы");
    assert_eq!(r.unrated, 0, "у всех рёбер записи (V2, V3) есть курс");
    assert!(!r.capped);
    assert_eq!(r.paths.len(), n);
    let best = scored.iter().map(|x| x.1).max();
    assert_eq!(r.best, best, "лучший курс круга — наибольший из всех");
    let deep = scored.iter().filter(|x| x.0).count() as u64;
    assert_eq!(r.deep, deep, "счёт циклов с глубокими сторонами");
    // Курсы отобранных — по убыванию и равны курсу их пути.
    for (path, rate) in r.paths.iter().zip(&r.rates) {
        assert_eq!(*rate, rate_of(&p, path));
    }
    let ranks: Vec<(bool, Decimal)> = r
        .paths
        .iter()
        .map(|c| (deep_of(&p, c), rate_of(&p, c)))
        .collect();
    assert!(ranks.windows(2).all(|w| w[0] >= w[1]), "лучшие первыми");
    // Каждый отобранный не хуже любого неотобранного (то есть отобраны лучшие, а не первые по перебору).
    let picked: Vec<_> = r.paths.iter().map(key).collect();
    let worst = *ranks.last().unwrap();
    for (d, rate, k) in &scored {
        if !picked.contains(k) {
            assert!(
                (*d, *rate) <= worst,
                "неотобранный цикл ({d}, {rate}) лучше отобранного {worst:?}"
            );
        }
    }
    let above = scored.iter().filter(|x| x.1 > Decimal::ONE).count() as u64;
    assert_eq!(r.above_one, above, "счёт циклов с курсом круга > 1");
    // Первые n по перебору (как до решения 404) — для сравнения в выводе теста.
    let first: Vec<_> = all.iter().take(n).map(|c| rate_of(&p, c)).collect();
    println!(
        "t189: M = {m}, первые {n} по перебору — курсы {first:?}, отобранные — {:?}",
        r.rates
    );
}

#[tokio::test]
async fn ranking_is_deterministic() {
    let (g1, p1) = graph_and_parts(false).await;
    let (g2, p2) = graph_and_parts(true).await;
    let all = route_paths::enumerate_cycles(&g1, USDT, 4, 10_000).len();
    let a = top(&g1, &p1, all, None);
    let b = top(&g2, &p2, all, None);
    let c = top(&g1, &p1, all, None);
    let ka: Vec<_> = a.paths.iter().map(key).collect();
    let kb: Vec<_> = b.paths.iter().map(key).collect();
    let kc: Vec<_> = c.paths.iter().map(key).collect();
    assert_eq!(ka.len(), all);
    assert_eq!(ka, kb, "другой порядок пулов в реестре — тот же отбор");
    assert_eq!(ka, kc, "повторный вызов — тот же отбор");
    assert_eq!(a.rates, b.rates);
    // Отбор N — начало полного порядка.
    for n in 1..all {
        let kn: Vec<_> = top(&g2, &p2, n, None).paths.iter().map(key).collect();
        assert_eq!(
            kn,
            ka[..n].to_vec(),
            "отбор {n} — первые {n} полного порядка"
        );
    }
}

#[tokio::test]
async fn rank_cap_stops_and_marks() {
    let (g, p) = graph_and_parts(false).await;
    let all = route_paths::enumerate_cycles(&g, USDT, 4, 10_000).len() as u64;
    let r = top(&g, &p, 100, Some(2));
    assert_eq!(r.seen, 2, "просмотрено ровно предел");
    assert!(r.capped, "перебор остановлен пределом");
    assert!(r.paths.len() <= 2);
    let full = top(&g, &p, 100, Some(all));
    assert!(!full.capped, "предел = числу циклов — не достигнут");
    assert_eq!(full.seen, all);
}

#[tokio::test]
async fn lost_reason_is_written() {
    let (g, p) = graph_and_parts(false).await;
    let r = top(&g, &p, 3, None);
    let mut t = Tally::new("USDT", &r, 3);
    // Пул не допущен: проверок (сверка с квотером, газ пробы) на этом окружении нет — та же причина, что на боте у
    // непроверенного пула; раньше она выбрасывалась (cycle_scan.rs signal: .ok()?).
    let q = p.quoting();
    let path = &r.paths[0];
    let dec: HashMap<Address, u8> = path
        .steps
        .iter()
        .flat_map(|s| [(s.token_in, 18u8), (s.token_out, 18u8)])
        .collect();
    let why = match cycle::legs_for_cycle(&q, path, 18, &dec) {
        Err(w) => w,
        Ok(_) => panic!("без проверок пул не допускается"),
    };
    assert!(!why.is_empty());
    t.lost(Lost::Legs(why.clone()));
    t.lost(Lost::Cut);
    let (text, data) = t.line();
    assert!(text.contains("пул не допущен 1"), "{text}");
    assert!(text.contains("строгий верх ρ < 1 1"), "{text}");
    assert!(
        text.contains(&format!("просмотрено циклов {}", r.seen)),
        "{text}"
    );
    assert_eq!(
        data["examples"]["legs"],
        json!(why),
        "пример причины — в данных строки"
    );
    assert_eq!(data["lost"]["legs"], json!(1));

    // Неисполнимый сигнал (исполнителя у кошелька нет, бандл не собирается без каналов) — в строке отбора ядра.
    // Сигнал строится на двухшаговом цикле USDT → CAKE → USDT (оба пула CAKE-USDT с картой тиков слов −3…3) — как в
    // tests/bsc_dex_cycle.rs evaluates_cycle_profit_model_85: на нём вход 100 USDT исполняется целиком. Лучший по курсу
    // цикл (r.paths[0]) идёт через пул с узким окном записанных тиков — route_calc.rs complete: «шаг маршрута не
    // исполним целиком» (100 USDT не помещаются в записанную ликвидность), это граница записи, а не формы исполнения.
    let two = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    let pair = two.first().expect("есть двухшаговый цикл");
    let legs: Vec<Leg> = pair
        .steps
        .iter()
        .map(|s| Leg {
            entry: s.edge.entry.clone(),
            usable: Usable {
                gas: 150_000,
                hook_pct: Decimal::ZERO,
                tax_pct: Decimal::ZERO,
                hook: Default::default(),
            },
            token_in: s.token_in,
            token_out: s.token_out,
            dec_in: 18,
            dec_out: 18,
        })
        .collect();
    let calc = cycle::eval(&p.ctx, &legs, Decimal::new(100, 0)).expect("цикл считается на записи");
    let sig = cycle::build_signal(
        &p.ctx,
        USDT,
        "USDT",
        &calc,
        &legs,
        U256::from(1_000_000_000u64),
        Decimal::ZERO,
        Some(Decimal::new(600, 0)),
    )
    .expect("сигнал собирается");
    assert!(!sig.executable);
    let dc =
        arb::venues::bsc_dex::quote::cycle_view::to_dex_cycle(&VenueId("bsc_dex".into()), &sig);
    let (text, data) = arb::core::cycle_scan_log::cands_line(&[dc], 7);
    assert!(
        text.contains("проход поиска 7 мс, сигналов 1, неисполнимых 1"),
        "{text}"
    );
    assert!(text.contains("кандидатов в план 0"), "{text}");
    assert_eq!(data["not_executable_example"], json!(sig.exec_note));
}
