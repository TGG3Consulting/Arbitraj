//! Файл: tests/uncapped_maxwin.rs
//! Что делает: решение 245 (часть Б) — окно максимального объёма кандидата (core/uncapped_maxwin.rs), длительность
//! сделок (core/deal_time.rs) и «успели бы» (core/maxwin_view.rs) на настоящем ядре (адаптеры BingX и DEX без сети),
//! настоящей SQLite (таблица uncapped, окна решения 125) и записанных данных: расчёт «без потолка» — функции ядра на
//! записи CAKE (tests/size_common: стакан BingX, пул CAKE/USDT V2, как tests/report_missed.rs); длительности сделок —
//! из учёта бота 05.11 (tests/data/report_2026-11-05/deals.csv: длительность и площадки 400 сделок).
//! Проверяется: выгодные расчёты подряд — одно окно (начало, пик, последний выгодный); невыгодный расчёт (без упущенного,
//! в базу не пишется) закрывает окно в строке эпизода (конец, точная длительность); новое окно в том же эпизоде — n = 2;
//! промежуток длиннее uncapped.maxwin_gap_ms — окно оборвалось, следующее — новое; самое длинное остаётся «best»; то же —
//! в сводке открытого окна решения 125; медиана и p75 сделок пары; мало сделок пары — по бирже-источнику; мало и там —
//! «нет данных»; «успели бы»; столбцы отчёта «Упущено»; запись без окна — «нет данных». Дополнение 09.11 (maxwin_watch):
//! наблюдение uncapped.maxwin_watch_ms — «держалось всё наблюдение», цепочка расчётов наблюдения в очереди.
//! Дополнение п. 2 (deal_time_from_decision): длительность сделки — от решения о сделке, старые — от открытия.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{uncapped_store, uncapped_maxwin, maxwin_view, deal_time, window_run},
//! arb::report::missed_rows, tests/size_common.

mod size_common;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::engine::lk;
use arb::core::size_uncapped::{At, uncapped};
use arb::core::types::{Direction, Side, TradeRequest, TradeResult, TradeStatus, VenueId};
use arb::core::uncapped_state::Key;
use arb::core::uncapped_types::{BASIS_LIVE, Battle, Episode, Record, missed};
use arb::core::{uncapped_store, window_run};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::uncapped_db::Filter;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use size_common::curve::{Dir, RecCurve};
use size_common::{Rec, d, params};
use tokio::sync::broadcast;

/// Настоящее ядро с адаптерами BingX и DEX (без сети), как tests/window_dex_price.rs.
async fn start(d: &std::path::Path) -> (Core, Store) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let raw = arb::storage::raw_log::RawLog::start(d.to_path_buf(), settings.clone());
    let bx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let dx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
    };
    let gw = Registry::new();
    gw.register(std::sync::Arc::new(arb::venues::bingx::Bingx::new(bx)))
        .await;
    gw.register(std::sync::Arc::new(arb::venues::bsc_dex::BscDex::new(dx)))
        .await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let deps = Deps {
        gw,
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    };
    (Core::start(deps).await, settings)
}

/// Направление записей кандидата: bsc_dex → bingx (на записи CAKE без потолка проходит пороги, как отказ в
/// tests/report_missed.rs).
fn back() -> Direction {
    let d = size_common::calc::direction();
    Direction {
        asset: d.asset,
        buy_on: d.sell_on,
        sell_on: d.buy_on,
    }
}

/// Запись кандидата «без потолка» по расчёту ядра на записанных данных (bsc_dex → bingx, бюджет 20 USDT, отказ):
/// порог «проходит» — floor. Время записи — at.
async fn record(rec: &Rec, floor: Decimal, at: DateTime<Utc>) -> Record {
    let mut c = RecCurve {
        rec,
        dir: Dir::DexToBingx,
        slip: Decimal::ONE,
    };
    let p = params(rec, Dir::DexToBingx, d("1000000"));
    let u = uncapped(
        &mut c,
        &p,
        d("20"),
        &[d("20"), d("100")],
        |pt| pt.profit >= floor,
        None,
    )
    .await
    .unwrap();
    let opt = At::of(&u.opt);
    let z = Decimal::ZERO;
    Record {
        at,
        direction: back(),
        battle: Battle {
            verdict: "reject".into(),
            reason: Some("прибыль ниже порога".into()),
            deal_id: None,
            dry: false,
            qty: z,
            invested: z,
            profit: z,
            budget: Some(d("20")),
            plan_budget: None,
            free: None,
            parts: None,
            window_from: None,
        },
        parts: None,
        limits_error: None,
        budget: d("20"),
        limited_by: "per_trade".into(),
        min_profit_usd: floor,
        min_profit_pct: z,
        opt,
        opt_passes: u.opt_passes,
        opt_profit_pct: z,
        opt_buy_avg: z,
        opt_sell_avg: z,
        opt_proceeds: z,
        opt_costs_usdt: z,
        opt_transit_s: z,
        opt_costs: vec![],
        min_plus: u.min_plus.as_ref().map(At::of),
        at_budget: u.at_budget.clone(),
        missed: missed("reject", u.opt_passes, opt.profit, &u.at_budget),
        levels: u.levels.clone(),
        upper_qty: u.upper_qty,
        search_cap: d("1000000"),
        cap_reached: u.cap_reached,
        limit: u.limit.clone(),
        plan_gap: None,
        evals: u.evals,
        peaks: u.peaks,
        capped: u.capped,
        ms: 0.0,
        ep: Episode {
            first_at: at,
            last_at: at,
            count: 1,
            capped_count: 0,
            peak_at: at,
        },
        basis: BASIS_LIVE.into(),
        decided_at: None,
        snapshot: None,
    }
}

/// Сделки учёта 05.11 (deals.csv): площадки по коду, длительность — от открытия до продажи. Возвращает: сделки и
/// длительности пары bingx → bsc_dex, мс.
fn ledger_deals() -> (Vec<Deal>, Vec<i64>) {
    let v = |c: char| match c {
        'B' => "bingx",
        'D' => "bsc_dex",
        'M' => "mexc",
        'K' => "kucoin",
        _ => "gate",
    };
    let raw = std::fs::read_to_string("tests/data/report_2026-11-05/deals.csv").unwrap();
    let t0 = Utc::now() - Duration::days(1);
    let (mut out, mut bd) = (vec![], vec![]);
    for (i, l) in raw.lines().filter(|l| !l.is_empty()).enumerate() {
        let f: Vec<&str> = l.split(',').collect();
        let c: Vec<char> = f[3].chars().collect();
        let secs: i64 = f[1].parse().unwrap();
        let dir = Direction {
            asset: size_common::calc::direction().asset,
            buy_on: VenueId(v(c[0]).into()),
            sell_on: VenueId(v(c[1]).into()),
        };
        let trade = |side: Side, at: DateTime<Utc>| TradeResult {
            request: TradeRequest {
                venue: dir.buy_on.clone(),
                asset: dir.asset.clone(),
                side,
                qty: d("1"),
                worst_price: d("1"),
                deal_id: format!("csv{i}"),
                route: None,
                exit_position: false,
                remeasure: None,
            },
            status: TradeStatus::Filled,
            filled_qty: d("1"),
            avg_price: d("1"),
            fees: vec![],
            venue_ref: format!("csv{i}"),
            chain: None,
            finished_at: at,
            executor: None,
        };
        let (buy, sell) = (
            trade(Side::Buy, t0),
            trade(Side::Sell, t0 + Duration::seconds(secs)),
        );
        let mut deal = Deal::new(format!("csv{i}"), dir.clone(), json!({}), Decimal::ZERO);
        deal.started_at = t0;
        deal.buy = Some(buy);
        deal.sells = vec![sell];
        deal.step = Step::Closed;
        if f[3] == "BD" {
            bd.push(secs * 1000);
        }
        out.push(deal);
    }
    bd.sort_unstable();
    (out, bd)
}

/// Тело единственной записи кандидата из базы (страница экрана) и её показ.
async fn row(core: &Core) -> (Value, Value) {
    let f = Filter {
        from: None,
        to: None,
        kind: Some("candidate".into()),
        missed_only: false,
    };
    let r = core.uncapped_list(f, 10, 0).await.unwrap();
    assert_eq!(r["total"], 1, "один эпизод");
    let x = &r["rows"][0];
    (x["body"].clone(), x["maxwin_view"].clone())
}

#[tokio::test]
async fn maxwin_windows_and_deal_time() {
    let dir = std::env::temp_dir().join(format!("arb-maxwin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (core, s) = start(&dir).await;
    let set = |n: &'static str, v: Value| {
        let s = s.clone();
        async move { s.set(n, Scope::Global, Some(v), "тест").await.unwrap() }
    };
    for (n, v) in [
        ("uncapped.episode_gap_ms", 120_000),
        ("uncapped.keep_days", 30),
        ("window.min_hold_ms", 0),
        ("window.queue_max", 100),
        ("window.max_mb", 10),
        ("window.keep_days", 7),
        ("window.zstd_level", 3),
        ("window.cleanup_s", 3600),
    ] {
        set(n, json!(v)).await;
    }
    set("window.enabled", json!(true)).await;
    let rec = Rec::load("uncapped-maxwin").await;
    // Окно решения 125 направления — открыто до расчётов.
    let mut calc = size_common::calc::calc(&rec, d("10"));
    calc.direction = back();
    window_run::pass(&core, &calc, true);
    for _ in 0..50 {
        let w = core.windows(Default::default(), 10, 0).await.unwrap();
        if w["total"] == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let (deals, bd) = ledger_deals();
    lk(&core.inner.deals).extend(deals.into_iter().map(|x| (x.id.clone(), x)));
    let dr = size_common::calc::direction();
    let (bingx, dex) = (dr.buy_on.clone(), dr.sell_on.clone());

    // Настройки не заданы — окна не ведутся, медианы нет (причины).
    let ctx = core.maxwin_ctx();
    assert!(
        ctx.gap
            .as_ref()
            .unwrap_err()
            .contains("uncapped.maxwin_gap_ms")
    );
    let v = ctx.view(&bingx, &dex, None);
    assert_eq!((v.status, v.would), ("none", "unknown"));
    assert!(v.stat.unwrap_err().contains("uncapped.deal_time_min_n"));
    set("uncapped.maxwin_gap_ms", json!(60_000)).await;
    set("uncapped.deal_time_min_n", json!(5)).await;

    // Медиана и p75 пары bingx → bsc_dex — линейная интерполяция по 298 сделкам; bingx → mexc (2) — по источнику; gate (1) — нет.
    let ctx = core.maxwin_ctx();
    let st = ctx.times.stat(&bingx, &dex, 5).unwrap();
    assert_eq!((st.n, st.by), (298, "pair"));
    let n = bd.len();
    assert_eq!(n, 298);
    assert_eq!(st.median_ms, (bd[n / 2 - 1] + bd[n / 2]) / 2);
    let pos = 0.75 * (n - 1) as f64;
    let p75 =
        bd[pos as usize] as f64 + (bd[pos as usize + 1] - bd[pos as usize]) as f64 * pos.fract();
    assert_eq!(st.p75_ms, p75.round() as i64);
    let src = ctx.times.stat(&bingx, &VenueId("mexc".into()), 5).unwrap();
    assert_eq!((src.n, src.by), (300, "source"));
    let none = ctx
        .times
        .stat(&VenueId("gate".into()), &dex, 5)
        .unwrap_err();
    assert!(none.starts_with("нет данных"), "{none}");

    // Расчёты кандидата: выгодные подряд — одно окно; невыгодный закрывает; снова выгодный — второе окно.
    let t0 = Utc::now() + Duration::seconds(1);
    let zero = Decimal::ZERO;
    let r1 = record(&rec, zero, t0).await;
    assert!(
        r1.opt_passes && r1.missed > zero,
        "запись: без потолка проходит, упущено есть"
    );
    let key = Key::Cand(back());
    let db_st = ctx.times.stat(&dex, &bingx, 5).unwrap();
    assert_eq!((db_st.n, db_st.by), (16, "pair"));
    uncapped_store::save(&core, &key, r1.clone()).await;
    let (b, _) = row(&core).await;
    assert_eq!(b["maxwin"]["n"], 1);
    assert_eq!(
        b["maxwin"]["best"]["start"]["invested"],
        json!(r1.opt.invested)
    );
    uncapped_store::save(
        &core,
        &key,
        record(&rec, zero, t0 + Duration::seconds(30)).await,
    )
    .await;
    let off = record(&rec, d("1000000000"), t0 + Duration::seconds(60)).await;
    assert!(
        !off.opt_passes && off.missed == zero,
        "невыгодный расчёт — без упущенного"
    );
    uncapped_store::save(&core, &key, off).await;
    let (b, view) = row(&core).await;
    let best: arb::core::uncapped_maxwin::MaxWin =
        serde_json::from_value(b["maxwin"]["best"].clone()).unwrap();
    assert_eq!(best.start_at, r1.at);
    assert_eq!(best.last_pass_at, t0 + Duration::seconds(30));
    assert_eq!(best.end_at, Some(t0 + Duration::seconds(60)));
    assert_eq!(
        (best.end.as_deref(), best.evals, best.ms()),
        (Some("not_passing"), 2, 60_000)
    );
    assert_eq!(
        (view["status"].as_str(), view["exact"].as_bool()),
        (Some("ended"), Some(true))
    );
    let would = if 60_000 >= db_st.median_ms {
        "yes"
    } else {
        "no"
    };
    assert_eq!(view["would"], would, "{view}");
    assert_eq!(view["deal_time"]["n"], 16);

    // Второе окно в том же эпизоде; промежуток длиннее 60 с — оборвалось, третье — новое. Самое длинное — первое.
    uncapped_store::save(
        &core,
        &key,
        record(&rec, zero, t0 + Duration::seconds(90)).await,
    )
    .await;
    uncapped_store::save(
        &core,
        &key,
        record(&rec, zero, t0 + Duration::seconds(151)).await,
    )
    .await;
    let (b, _) = row(&core).await;
    assert_eq!(b["maxwin"]["n"], 3);
    assert_eq!(b["maxwin"]["best"]["start_at"], json!(t0));
    assert_eq!(
        b["maxwin"]["last"]["start_at"],
        json!(t0 + Duration::seconds(151))
    );

    // Сводка открытого окна решения 125 — те же окна направления.
    let w = core.windows(Default::default(), 10, 0).await.unwrap();
    let m = &w["windows"][0]["maxwin_view"][0];
    assert_eq!(
        (m["buy_on"].as_str(), m["wins"]["n"].as_u64()),
        (Some("bsc_dex"), Some(3))
    );
    assert_eq!(m["view"]["status"], "ended");

    // Отчёт «Упущено»: столбцы окна у эпизода.
    let data = core.uncapped_report(None, None).await.unwrap();
    let c = &arb::report::missed_rows::candidates(&data)[0].maxwin;
    assert_eq!(
        (c.secs, c.kind.as_str(), c.n),
        (Some(60.0), "точно", Some(3.0))
    );
    assert_eq!(c.start, arb::report::missed_rows::when(t0));
    assert_eq!(
        c.deal_by,
        "пара, 16 сделок, из них 16 — от открытия (момента решения в учёте нет)"
    );
    assert_eq!(c.would, if would == "yes" { "да" } else { "нет" });
    arb::report::missed(&data, None, None, Utc::now()).expect("книга «Упущено» с новыми столбцами");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ожидающая работа ключа кандидата направления: (есть, расчёт наблюдения ли). Ошибок нет.
fn pending(core: &Core, key: &Key) -> Option<bool> {
    lk(&core.inner.uncapped.slots)
        .get(key)
        .and_then(|s| s.pending.as_ref().map(|j| j.watch))
}

/// Решение 245, дополнение (Тигран 09.11): наблюдение окна uncapped.maxwin_watch_ms на тех же записях CAKE.
/// Проверяется: выгодный расчёт через ≥ времени наблюдения закрывает окно «держалось всё наблюдение» (watch_ms,
/// статус held, «не меньше», «успели бы» по медиане); пока направление выгодно, новое окно не начинается; невыгодный
/// расчёт снимает это, следующее выгодное — новое окно; расчёт наблюдения окна не начинает; окно кончилось раньше
/// наблюдения — точная длительность; цепочка расчётов наблюдения — очередь «без потолка» (тот же ключ, ожидающий
/// кандидат не вытесняется, кандидат вытесняет наблюдение); монета занята сделкой — расчёта наблюдения нет;
/// отчёт «Упущено» — «держалось всё наблюдение».
#[tokio::test]
async fn maxwin_watch() {
    let dir = std::env::temp_dir().join(format!("arb-maxwin-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (core, s) = start(&dir).await;
    let set = |n: &'static str, v: Value| {
        let s = s.clone();
        async move { s.set(n, Scope::Global, Some(v), "тест").await.unwrap() }
    };
    for (n, v) in [
        ("uncapped.episode_gap_ms", 600_000),
        ("uncapped.keep_days", 30),
        ("uncapped.maxwin_gap_ms", 45_000),
        ("uncapped.maxwin_watch_ms", 60_000),
        ("uncapped.deal_time_min_n", 5),
    ] {
        set(n, json!(v)).await;
    }
    let (deals, _) = ledger_deals();
    lk(&core.inner.deals).extend(deals.into_iter().map(|x| (x.id.clone(), x)));
    let rec = Rec::load("uncapped-maxwin").await;
    let zero = Decimal::ZERO;
    let key = Key::Cand(back());
    let t0 = Utc::now() + Duration::seconds(1);
    let sec = |s: i64| t0 + Duration::seconds(s);

    // Расчёт наблюдения окна не начинает.
    arb::core::uncapped_watch::saved(&core, &back(), record(&rec, zero, sec(-100)).await).await;
    let going = lk(&core.inner.uncapped.maxwin)
        .get(&back())
        .and_then(|t| t.going());
    assert_eq!(going, None, "расчёт наблюдения нового окна не начинает");
    assert_eq!(lk(&core.inner.uncapped.stats).watched, 1);

    // Окно с t0; через 30 с — расчёт наблюдения продолжает окно; через 60 с — держалось всё наблюдение.
    uncapped_store::save(&core, &key, record(&rec, zero, t0).await).await;
    arb::core::uncapped_watch::saved(&core, &back(), record(&rec, zero, sec(30)).await).await;
    uncapped_store::save(&core, &key, record(&rec, zero, sec(60)).await).await;
    let (b, view) = row(&core).await;
    let best: arb::core::uncapped_maxwin::MaxWin =
        serde_json::from_value(b["maxwin"]["best"].clone()).unwrap();
    assert_eq!(
        (
            best.end.as_deref(),
            best.watch_ms,
            best.end_at,
            best.ms(),
            best.evals
        ),
        (Some("watch"), Some(60_000), None, 60_000, 3)
    );
    assert_eq!(
        (view["status"].as_str(), view["exact"].as_bool()),
        (Some("held"), Some(false))
    );
    let med = core
        .maxwin_ctx()
        .times
        .stat(&back().buy_on, &back().sell_on, 5)
        .unwrap()
        .median_ms;
    let would = if 60_000 >= med { "yes" } else { "unknown" };
    assert_eq!(view["would"], would, "{view}");
    // Отчёт «Упущено»: держалось всё наблюдение.
    let data = core.uncapped_report(None, None).await.unwrap();
    let c = &arb::report::missed_rows::candidates(&data)[0].maxwin;
    assert_eq!(
        (c.secs, c.kind.as_str()),
        (Some(60.0), "не меньше (держалось всё наблюдение)")
    );
    assert!(
        c.end.starts_with("держалось всё наблюдение (60 с+)"),
        "{}",
        c.end
    );

    // Всё ещё выгодно — нового окна нет; невыгодный снимает; следующее выгодное — новое окно (n = 2), кончилось
    // раньше наблюдения — точно.
    uncapped_store::save(&core, &key, record(&rec, zero, sec(90)).await).await;
    let (b, _) = row(&core).await;
    assert_eq!(b["maxwin"]["n"], 1, "держалось и выгодно — то же окно");
    let off = d("1000000000");
    uncapped_store::save(&core, &key, record(&rec, off, sec(120)).await).await;
    uncapped_store::save(&core, &key, record(&rec, zero, sec(130)).await).await;
    uncapped_store::save(&core, &key, record(&rec, off, sec(150)).await).await;
    let (b, _) = row(&core).await;
    assert_eq!(b["maxwin"]["n"], 2);
    let last: arb::core::uncapped_maxwin::MaxWin =
        serde_json::from_value(b["maxwin"]["last"].clone()).unwrap();
    assert_eq!(
        (last.end.as_deref(), last.ms(), last.watch_ms),
        (Some("not_passing"), 20_000, None)
    );

    // Цепочка расчётов наблюдения: окно идёт (начало — сейчас) — next ставит расчёт наблюдения в очередь «без потолка».
    for (n, v) in [
        ("uncapped.min_interval_ms", 600_000),
        ("uncapped.queue_max", 8),
        ("uncapped.max_concurrent", 1),
    ] {
        set(n, json!(v)).await;
    }
    set("uncapped.enabled", json!(true)).await;
    let now = Utc::now();
    uncapped_store::save(&core, &key, record(&rec, zero, now).await).await;
    let battle = record(&rec, zero, now).await.battle;
    // Монета занята нашей сделкой — расчёта наблюдения нет.
    let coin = arb::core::coin_key::CoinKey::of(&back().asset);
    lk(&core.inner.active).insert(coin.clone());
    arb::core::uncapped_watch::next(&core, &back(), battle.clone());
    assert!(!lk(&core.inner.uncapped.slots).contains_key(&key));
    lk(&core.inner.active).remove(&coin);
    // Свободна — расчёт наблюдения идёт (живого рынка у ядра теста нет — сбой расчёта), после него следующий расчёт
    // наблюдения ждёт интервала ключа.
    let failed0 = lk(&core.inner.uncapped.stats).failed;
    arb::core::uncapped_watch::next(&core, &back(), battle.clone());
    for _ in 0..100 {
        if pending(&core, &key).is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(lk(&core.inner.uncapped.stats).failed > failed0);
    assert_eq!(
        pending(&core, &key),
        Some(true),
        "следующий расчёт наблюдения"
    );
    // Кандидат вытесняет ожидающее наблюдение; наблюдение ожидающего кандидата не вытесняет.
    arb::core::uncapped_queue::submit(&core, key.clone(), &back(), battle.clone(), None);
    assert_eq!(pending(&core, &key), Some(false));
    arb::core::uncapped_queue::submit_watch(&core, &back(), battle);
    assert_eq!(pending(&core, &key), Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Решение 245, дополнение п. 2 (Тигран 09.11): длительность сделки — от решения о сделке (Deal::decided_at), у сделок
/// без него — от открытия (их число — Stat.from_open). Сделки учёта 05.11 (deals.csv): у первых 100 сделок пары
/// bingx → bsc_dex момент решения — за 10 с до открытия.
#[test]
fn deal_time_from_decision() {
    let (mut deals, _) = ledger_deals();
    let dr = size_common::calc::direction();
    let (bingx, dex) = (dr.buy_on.clone(), dr.sell_on.clone());
    let pair = |x: &Deal| x.direction.buy_on == bingx && x.direction.sell_on == dex;
    let mut want = vec![];
    let mut k = 0;
    for x in deals.iter_mut().filter(|x| pair(x)) {
        let end = x.sells[0].finished_at;
        if k < 100 {
            x.decided_at = Some(x.started_at - Duration::seconds(10));
            k += 1;
        }
        want.push((end - x.decided_at.unwrap_or(x.started_at)).num_milliseconds());
        assert_eq!(arb::core::deal_time::duration_ms(x), want.last().copied());
    }
    want.sort_unstable();
    let t = arb::core::deal_time::DealTimes::of(deals.iter());
    let st = t.stat(&bingx, &dex, 5).unwrap();
    assert_eq!((st.n, st.by, st.from_open), (298, "pair", 198));
    assert_eq!(st.median_ms, arb::core::deal_time::quantile(&want, 0.5));
    assert_eq!(st.p75_ms, arb::core::deal_time::quantile(&want, 0.75));
    let src = t.stat(&bingx, &VenueId("mexc".into()), 5).unwrap();
    assert_eq!((src.n, src.by, src.from_open), (300, "source", 200));
    // Сериализация учёта: поле есть только у сделок с моментом решения; старая запись читается без него.
    let x = deals.iter().find(|x| x.decided_at.is_some()).unwrap();
    let back: Deal = serde_json::from_value(serde_json::to_value(x).unwrap()).unwrap();
    assert_eq!(back.decided_at, x.decided_at);
    let mut old = serde_json::to_value(&deals[0]).unwrap();
    old.as_object_mut().unwrap().remove("decided_at");
    let old: Deal = serde_json::from_value(old).unwrap();
    assert_eq!(old.decided_at, None);
}
