//! Файл: tests/core_precheck_fast_266.rs
//! Что проверяет: решение 266 (быстрая предпроверка) на настоящем ядре с SQLite во временной папке (реестр шлюза
//! пуст, виды площадок — в inner.kinds, как tests/core_daily_pair.rs; моков нет) и записанных данных CAKE
//! (tests/size_common: стакан BingX 23.10, правила пары и сеть BEP20 BingX 24.10, пул CAKE/USDT V2):
//! 1) объяснение выбора быстрого пути (precheck_fast::choice) на расчёте по записям — честные поля (один расчёт, пиков
//!    нет, границы — сам объём, бюджет — бюджет пересчёта), текст без «оптимум» и «посчитано объёмов»; ужатый объём;
//! 2) быстрый путь не дал плана (финальный расчёт отказал: площадки не подключены; объём меньше шага) — причина в
//!    запросе, строка «План: … не прошёл (причина) — выполнен полный подбор»; пересчёт с объёмом кандидата, не
//!    дошедший до быстрого пути, даёт тот же итог, что без объёма (побайтно, JSON);
//! 3) настройки не заданы — режим выключен, предпроверка идёт прежним путём: «(время: …)» прежнего вида, без «План:»
//!    и без пометки «одновременно»; включение на лету; объём кандидата — Calc::qty;
//! 4) precheck.parallel = да — первая по порядку причина отказа и её текст те же, что по очереди (биржа → DEX, DEX →
//!    биржа, биржа → биржа); уведомление risk.low_bnb — столько же раз (только когда ошибка BNB первая по порядку);
//! 5) exec.via_kyber = да на DEX направления — быстрый путь не применяется (финальный расчёт не запускается);
//! 6) precheck.networks_memory: записи монеты в памяти моложе интервала фонового опроса (venue.networks_poll_ms, не
//!    задана — venue.status_poll_ms) — запроса к бирже нет (биржа не подключена, а шаг сетей проходит); вывод в памяти
//!    закрыт — тот же текст отказа «… вывод закрыт»; записи старше интервала, интервал не задан, нет сети с адресом
//!    монеты — запрос (отказ «биржа не подключена», как без настройки); источник — в «Время по шагам»;
//! 7) precheck.balance_first: баланс не получен у биржи A — не отказ (Read::Missed, строка «Свободный баланс на X не
//!    получен…» в тексте журнала, дальше расчёт), у DEX A — отказ с причиной; критерий «свободного меньше, чем нужно
//!    кандидату»; списание кандидата больше предела расчёта — быстрый путь не применяется; текст отказа «свободный
//!    баланс …: свободно …, кандидату нужно …»;
//! 8) все настройки «нет», все «да» и каждая по одной — предпроверка даёт ту же первую по порядку причину отказа.
//!
//! Часть 4 с ПОЛУЧЕННЫМ балансом (хватает → быстрый путь; меньше → полный подбор под остаток; исполнитель не читает
//! баланс повторно) здесь не проверяется: баланс читает адаптер площадки (в тесте реестр шлюза пуст, записей ответа
//! балансов под контракт Venue::balance нет), а площадка-заглушка запрещена (CLAUDE.md).
//! Проход быстрого пути (объём кандидата выше порогов → план без подбора) здесь не проверяется: полный расчёт ядра
//! (costs::evaluate) на записанных данных не собирается — в записях нет ставок комиссии аккаунта, газа и σ
//! (tests/size_common/mod.rs), а площадка-заглушка запрещена (CLAUDE.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{precheck, precheck_fast, profit, risk}, tests/report_common, tests/size_common.

mod report_common;
mod size_common;

use arb::core::Core;
use arb::core::dirctx::DirCtx;
use arb::core::precheck::{self, replan, replan_sized};
use arb::core::precheck_balance::{self, BALANCE, Cand, Read};
use arb::core::precheck_fast::{self, FAST, Opts, PARALLEL, Path, Want, choice, recalc};
use arb::core::precheck_nets::{self, MEMORY, Src};
use arb::core::registry::Presence;
use arb::core::risk::Mode;
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use report_common::{dir, start};
use serde_json::json;
use size_common::{CAKE, Rec, USDT, d};

/// Площадка по id.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Направление по CAKE.
fn dir_of(buy: &str, sell: &str) -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        buy_on: v(buy),
        sell_on: v(sell),
    }
}

/// Ядро «В работе»: виды площадок, обязательные money.* и risk.* (как tests/core_daily_pair.rs core_at).
async fn core_at(name: &str) -> (Core, Store) {
    let (core, st) = start(&dir(name)).await;
    let kinds = [
        ("bingx", VenueKind::Cex),
        ("bsc_dex", VenueKind::Dex),
        ("mexc", VenueKind::Cex),
    ];
    *core.inner.kinds.write().unwrap() = kinds.iter().map(|(id, k)| (v(id), *k)).collect();
    st.venue_kinds()
        .set(kinds.iter().map(|(id, k)| (id.to_string(), *k)));
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.min_bnb_balance", "0.01"),
        ("risk.max_daily_loss_usd", "10"),
    ] {
        st.set(n, Scope::Global, Some(json!(x)), "тест")
            .await
            .unwrap();
    }
    for (id, _) in kinds {
        let sc = Scope::Venue(id.into());
        st.set("risk.max_errors_in_row", sc, Some(json!(3)), "тест")
            .await
            .unwrap();
    }
    core.set_mode(Mode::Running).await;
    (core, st)
}

/// Контекст направления bingx → bsc_dex на записанных данных (как tests/core_size_budget_early.rs).
fn cx(rec: &Rec) -> DirCtx {
    let p = |id: &str, info| Presence {
        venue: v(id),
        ticker: "CAKE".into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info,
        via: None,
    };
    DirCtx {
        dir: dir_of("bingx", "bsc_dex"),
        a: p("bingx", Some(rec.net.clone())),
        b: p("bsc_dex", None),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        cex: vec![],
        sigma: d("0.0005"),
        transit_s: d("20"),
        k: d("1"),
        h: d("0.7"),
        closure_share: None,
        return_per_trade: false,
        pair_a: Some(rec.pair.clone()),
        pair_b: None,
        usdt_b: None,
        transfer_tax: Some(rust_decimal::Decimal::ZERO),
    }
}

/// Причина отказа предпроверки без времени шагов («… (время: …)»).
fn reason(e: &str) -> &str {
    e.split(" (время: ").next().unwrap()
}

/// Сколько записей журнала об уведомлении «мало BNB» (событие включено, Telegram не настроен — запись «не отправлено»).
async fn low_bnb_notes(core: &Core) -> usize {
    let all = core.inner.deps.journal.list(500, None).await.unwrap();
    all.iter()
        .filter(|e| e.kind == "notify" && e.message.contains("не отправлено"))
        .count()
}

#[tokio::test]
async fn fast_choice_is_honest() {
    let rec = Rec::load("fast-266-choice").await;
    let c = size_common::calc::calc(&rec, d("10"));
    let s = choice(&c, d("10"), d("20"), "CAKE");
    assert_eq!((s.qty, s.profit, s.budget), (c.qty, c.profit_usd, d("20")));
    assert_eq!((s.min_qty, s.max_qty), (c.qty, c.qty));
    assert_eq!(s.at_budget_profit, c.profit_usd);
    assert_eq!(
        (s.evals, s.peaks, s.capped, s.max_exact),
        (1, 0, false, false)
    );
    assert!(
        s.why
            .starts_with("объём кандидата 10 CAKE пересчитан на свежих данных: прибыль "),
        "{}",
        s.why
    );
    assert!(
        s.why
            .ends_with("повторный подбор объёма не выполнялся (precheck.fast_recalc)")
    );
    for word in ["оптимум", "посчитано объёмов", "ужат"] {
        assert!(!s.why.contains(word), "«{word}»: {}", s.why);
    }
    // Финальный расчёт ужал объём (запрошено 12, расчёт — на 10): объём сделки — фактический, в тексте — как у полного пути.
    let s = choice(&c, d("12"), d("20"), "CAKE");
    assert_eq!(s.qty, d("10"));
    assert!(s.why.contains("объём кандидата 12 CAKE"), "{}", s.why);
    assert!(
        s.why.ends_with(
            "по финальной котировке объём ужат (бюджет или глубина исполнителя): 10 CAKE"
        ),
        "{}",
        s.why
    );
    // Строки хронологии.
    let line = |p: Path| {
        Want {
            path: Some(p),
            ..Want::new(d("10"))
        }
        .line()
        .unwrap()
    };
    assert_eq!(
        line(Path::Used),
        "План: объём кандидата пересчитан на свежих данных (без повторного подбора)"
    );
    assert_eq!(
        line(Path::Full("причина".into())),
        "План: пересчёт объёма кандидата не прошёл (причина) — выполнен полный подбор"
    );
    assert!(line(Path::Off("причина".into())).contains("не применялся (причина)"));
    assert_eq!(Want::new(d("10")).line(), None);
}

#[tokio::test]
async fn failed_fast_path_falls_back_to_full_search() {
    let (core, st) = core_at("fast-266-fallback").await;
    let rec = Rec::load("fast-266-fallback").await;
    let cx = cx(&rec);
    let min = (d("0.1"), d("0.1"));
    // Финальный расчёт отказал (площадки не подключены) — плана нет, причина названа, отказа нет: дальше полный подбор.
    let mut w = Want::new(d("10"));
    let got = Box::pin(recalc(&core, &cx, &mut w, d("20"), rec.pair.step_size, min)).await;
    assert!(got.is_none());
    let why = match w.path.clone() {
        Some(Path::Full(why)) => why,
        p => panic!("ожидался откат на полный подбор: {p:?}"),
    };
    assert!(!why.is_empty());
    let line = w.line().unwrap();
    assert!(line.contains("не прошёл") && line.contains(&why), "{line}");
    assert!(line.ends_with("выполнен полный подбор"), "{line}");
    // Объём кандидата меньше шага количества — расчёт не запускается.
    let mut w = Want::new(rec.pair.step_size / d("2"));
    let got = Box::pin(recalc(&core, &cx, &mut w, d("20"), rec.pair.step_size, min)).await;
    assert!(got.is_none());
    assert!(
        matches!(&w.path, Some(Path::Full(t)) if t.contains("не больше нуля")),
        "{:?}",
        w.path
    );
    // Решение 266 (Kyber): финальная котировка у внешнего исполнителя — быстрый путь не применяется вовсе.
    let sc = Scope::Venue("bsc_dex".into());
    st.set("exec.via_kyber", sc, Some(json!(true)), "тест")
        .await
        .unwrap();
    let mut w = Want::new(d("10"));
    let got = Box::pin(recalc(&core, &cx, &mut w, d("20"), rec.pair.step_size, min)).await;
    assert!(got.is_none());
    assert!(
        matches!(&w.path, Some(Path::Off(t)) if t.contains("exec.via_kyber")),
        "{:?}",
        w.path
    );
    assert!(w.line().unwrap().contains("не применялся"));
    // Пересчёт с объёмом кандидата, не дошедший до быстрого пути (монеты нет в справочнике), — тот же итог, что без объёма.
    let dd = dir_of("bingx", "bsc_dex");
    let plain = Box::pin(replan(&core, &dd)).await;
    let mut w = Want::new(d("10"));
    let sized = Box::pin(replan_sized(&core, &dd, None, Some(&mut w))).await;
    assert_eq!(
        serde_json::to_string(&plain).unwrap(),
        serde_json::to_string(&sized).unwrap()
    );
    assert_eq!(w.path, None, "до быстрого пути не дошли");
    let none = Box::pin(replan_sized(&core, &dd, None, None)).await;
    assert_eq!(
        serde_json::to_string(&plain).unwrap(),
        serde_json::to_string(&none).unwrap()
    );
}

#[tokio::test]
async fn unset_settings_keep_old_precheck() {
    let (core, st) = core_at("fast-266-unset").await;
    let rec = Rec::load("fast-266-unset").await;
    let cand = size_common::calc::calc(&rec, d("10"));
    let off = precheck_fast::read(&st, &cand);
    assert!(off.qty.is_none() && !off.parallel, "не заданы — выключено");
    assert!(!off.networks_memory && off.balance.is_none());
    let dd = dir_of("bingx", "bsc_dex");
    let e = Box::pin(precheck::run(&core, &dd, false, off, None))
        .await
        .unwrap_err();
    assert!(e.contains(" (время: "), "{e}");
    for new in ["План:", "одновременно", "Время по шагам"] {
        assert!(!e.contains(new), "«{new}»: {e}");
    }
    // Как до решения 266: Opts::default() — тот же отказ.
    let e0 = Box::pin(precheck::run(&core, &dd, false, Opts::default(), None))
        .await
        .unwrap_err();
    assert_eq!(reason(&e), reason(&e0));
    // На лету, без перезапуска; «нет» — выключено; объём кандидата — Calc::qty.
    for (name, val) in [
        (FAST, true),
        (PARALLEL, true),
        (MEMORY, true),
        (BALANCE, true),
    ] {
        st.set(name, Scope::Global, Some(json!(val)), "тест")
            .await
            .unwrap();
    }
    let on = precheck_fast::read(&st, &cand);
    assert_eq!(on.qty, Some(cand.qty));
    assert!(on.parallel && on.networks_memory);
    assert_eq!(on.balance, Some(Cand::of(&cand)));
    for name in [FAST, PARALLEL, MEMORY, BALANCE] {
        st.set(name, Scope::Global, Some(json!(false)), "тест")
            .await
            .unwrap();
    }
    let off = precheck_fast::read(&st, &cand);
    assert!(off.qty.is_none() && !off.parallel);
    assert!(!off.networks_memory && off.balance.is_none());
}

#[tokio::test]
async fn parallel_keeps_order_and_texts() {
    let (core, st) = core_at("fast-266-par").await;
    // Уведомление «мало BNB» включено: Telegram не настроен — каждая попытка оставляет запись журнала «не отправлено».
    let ev = Scope::Event("risk.low_bnb".into());
    st.set("notify.event.enabled", ev, Some(json!(true)), "тест")
        .await
        .unwrap();
    // Площадка без вида в реестре: её настройка задана — стоп-условия проходят, отказ даёт сам шаг «вид площадки».
    let sc = Scope::Venue("нет_такой".into());
    st.set("risk.max_errors_in_row", sc, Some(json!(3)), "тест")
        .await
        .unwrap();
    let par = Opts {
        parallel: true,
        ..Opts::default()
    };
    for (buy, sell) in [
        ("bingx", "bsc_dex"),
        ("bsc_dex", "bingx"),
        ("bingx", "mexc"),
        ("bsc_dex", "нет_такой"),
        ("нет_такой", "bingx"),
    ] {
        let dd = dir_of(buy, sell);
        let n0 = low_bnb_notes(&core).await;
        let seq = Box::pin(precheck::run(&core, &dd, false, Opts::default(), None))
            .await
            .unwrap_err();
        let n1 = low_bnb_notes(&core).await;
        let both = Box::pin(precheck::run(&core, &dd, false, par, None))
            .await
            .unwrap_err();
        let n2 = low_bnb_notes(&core).await;
        assert_eq!(reason(&seq), reason(&both), "{buy} → {sell}");
        assert_eq!(
            n1 - n0,
            n2 - n1,
            "уведомлений risk.low_bnb столько же: {buy} → {sell}"
        );
        assert!(n2 - n1 <= 1, "не больше одного: {buy} → {sell}");
        assert!(!seq.contains("одновременно"), "{seq}");
        // Пометка — только если запросы были (вид площадки A не найден — шагов нет, строка времени прежняя).
        let ran = !both.ends_with("(время: )");
        assert_eq!(
            ran,
            both.contains("шли одновременно (precheck.parallel)"),
            "{both}"
        );
        // Имена шагов — те же, в том же порядке (до первой ошибки включительно).
        let names = |e: &str| -> Vec<String> {
            let t = e.split(" (время: ").nth(1).unwrap_or_default();
            let t = t.split("; шаги сетей").next().unwrap_or_default();
            t.trim_end_matches(')')
                .split(", ")
                .filter(|p| !p.is_empty())
                .map(|p| p.rsplitn(3, ' ').nth(2).unwrap_or_default().to_string())
                .collect()
        };
        assert_eq!(names(&seq), names(&both), "{buy} → {sell}");
        println!("{buy} → {sell}: {seq}\n    {both}");
    }
    // Ошибка BNB первая по порядку (DEX на стороне A) — уведомление было; у «биржа → DEX» сторона A отказала раньше —
    // ошибка BNB стороны B не сообщается и уведомления нет.
    let dd = dir_of("bsc_dex", "bingx");
    let n0 = low_bnb_notes(&core).await;
    let _ = Box::pin(precheck::run(&core, &dd, false, par, None)).await;
    assert_eq!(low_bnb_notes(&core).await - n0, 1);
    let dd = dir_of("bingx", "bsc_dex");
    let n0 = low_bnb_notes(&core).await;
    let _ = Box::pin(precheck::run(&core, &dd, false, par, None)).await;
    assert_eq!(low_bnb_notes(&core).await - n0, 0);
}

/// Справочник сетей bingx в памяти ядра: запись CAKE (BEP20, записанный ответ BingX) с заданным возрастом и статусом вывода.
fn remember(core: &Core, rec: &Rec, age_ms: i64, withdraw: bool) {
    let mut net = rec.net.clone();
    net.venue = v("bingx");
    net.asset.bsc_contract = Some(CAKE);
    net.withdraw_enabled = withdraw;
    net.updated_at = chrono::Utc::now() - chrono::Duration::milliseconds(age_ms);
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(v("bingx"), std::sync::Arc::new(vec![net]));
}

#[tokio::test]
async fn networks_memory_skips_request_while_fresh() {
    let (core, st) = core_at("fast-266-nets").await;
    let rec = Rec::load("fast-266-nets").await;
    let ticker = rec.net.asset.ticker.clone();
    let dd = Direction {
        asset: Asset {
            ticker: ticker.clone(),
            bsc_contract: Some(CAKE),
        },
        buy_on: v("bingx"),
        sell_on: v("bsc_dex"),
    };
    let mem = Opts {
        networks_memory: true,
        ..Opts::default()
    };
    let sc = || Scope::Venue("bingx".into());
    remember(&core, &rec, 100, true);
    // Интервал фонового опроса не задан — памяти верить нельзя: запрос (биржа не подключена — тот же отказ, что без настройки).
    assert!(precheck_nets::recent(&core, &v("bingx"), &ticker, Some(CAKE)).is_none());
    let plain = Box::pin(precheck::run(&core, &dd, false, Opts::default(), None))
        .await
        .unwrap_err();
    let asked = Box::pin(precheck::run(&core, &dd, false, mem, None))
        .await
        .unwrap_err();
    assert_eq!(reason(&plain), reason(&asked));
    // venue.status_poll_ms — интервал, когда своя частота не задана (то же правило, что у фонового опроса).
    st.set("venue.status_poll_ms", sc(), Some(json!(60000)), "тест")
        .await
        .unwrap();
    let (list, age) = precheck_nets::recent(&core, &v("bingx"), &ticker, Some(CAKE)).unwrap();
    assert_eq!(list.len(), 1);
    assert!((100..60000).contains(&age), "{age}");
    // Своя частота задана — действует она: запись (100 мс) старше 50 мс — запрос.
    st.set("venue.networks_poll_ms", sc(), Some(json!(50)), "тест")
        .await
        .unwrap();
    assert!(precheck_nets::recent(&core, &v("bingx"), &ticker, Some(CAKE)).is_none());
    let asked = Box::pin(precheck::run(&core, &dd, true, mem, None))
        .await
        .unwrap_err();
    assert_eq!(reason(&plain), reason(&asked), "старая память — запрос");
    st.set("venue.networks_poll_ms", sc(), Some(json!(60000)), "тест")
        .await
        .unwrap();
    // Другой адрес контракта, другой тикер, время записи в будущем — запрос.
    assert!(precheck_nets::recent(&core, &v("bingx"), &ticker, Some(USDT)).is_none());
    assert!(precheck_nets::recent(&core, &v("bingx"), &ticker, None).is_none());
    assert!(precheck_nets::recent(&core, &v("bingx"), "НЕТ", Some(CAKE)).is_none());
    assert!(precheck_nets::recent(&core, &v("mexc"), &ticker, Some(CAKE)).is_none());
    remember(&core, &rec, -5000, true);
    assert!(precheck_nets::recent(&core, &v("bingx"), &ticker, Some(CAKE)).is_none());
    // Свежая память, вывод открыт: запроса к бирже нет — шаг сетей A пройден без подключённой биржи, отказ — дальше.
    remember(&core, &rec, 100, true);
    let passed = Box::pin(precheck::run(&core, &dd, true, mem, None))
        .await
        .unwrap_err();
    assert_ne!(reason(&plain), reason(&passed), "{passed}");
    assert!(!passed.contains("вывод закрыт"), "{passed}");
    println!("без настройки: {plain}\nиз памяти: {passed}");
    // Свежая память, вывод закрыт — тот же текст отказа, что даёт запрос.
    remember(&core, &rec, 100, false);
    for opts in [
        mem,
        Opts {
            parallel: true,
            ..mem
        },
    ] {
        let closed = Box::pin(precheck::run(&core, &dd, true, opts, None))
            .await
            .unwrap_err();
        assert_eq!(
            reason(&closed),
            format!("bingx: {ticker} вывод закрыт"),
            "{closed}"
        );
        // Память не тронута (писать нечего), источник — в подробном замере; настройка выключена — источника нет.
        assert!(
            closed.contains("(время: справочник сетей bingx "),
            "{closed}"
        );
    }
    assert_eq!(Src::Http.text(), "запрос");
    assert_eq!(Src::Memory(120).text(), "из памяти, возраст 120 мс");
}

#[tokio::test]
async fn balance_first_rules() {
    let (core, st) = core_at("fast-266-bal").await;
    let rec = Rec::load("fast-266-bal").await;
    let cand = Cand::of(&size_common::calc::calc(&rec, d("10")));
    // «Свободного меньше, чем нужно кандидату» — критерий refit::needed: списание по худшей цене больше предела.
    assert!(cand.spend_max > d("0"));
    assert!(cand.short(cand.spend_max - d("0.01")));
    assert!(!cand.short(cand.spend_max) && !cand.short(cand.spend_max + d("1")));
    // Биржа A баланс не отдала (биржа не подключена; venue.quote_asset не задана) — не отказ.
    let cex = dir_of("bingx", "bsc_dex");
    let why = match precheck_balance::read(&core, &cex, None).await {
        Ok(Read::Missed(why)) => why,
        r => panic!("ожидалось «не получен»: {r:?}"),
    };
    assert!(!why.is_empty());
    st.set(
        "venue.quote_asset",
        Scope::Venue("bingx".into()),
        Some(json!("USDT")),
        "тест",
    )
    .await
    .unwrap();
    let r = precheck_balance::read(&core, &cex, None).await;
    assert!(matches!(&r, Ok(Read::Missed(_))), "{r:?}");
    assert_eq!(r.unwrap().free(), None);
    let note = precheck_balance::missed_note(&cex, "причина");
    assert_eq!(
        note,
        "Свободный баланс на bingx не получен: причина — ордер на посчитанный объём без проверки баланса (решение 266)"
    );
    // DEX A: баланс кошелька не получен — отказ предпроверки с причиной.
    let dex = dir_of("bsc_dex", "bingx");
    let e = precheck_balance::read(&core, &dex, None).await.unwrap_err();
    assert!(
        e.starts_with("свободный баланс bsc_dex не получен: "),
        "{e}"
    );
    // Текст отказа «под остаток невыгодно»: причина — свободный баланс, сколько свободно и сколько нужно.
    let t = precheck_balance::short_reject(&cex, d("5"), "USDT", &cand, "прибыль ниже порога");
    assert!(
        t.starts_with("свободный баланс bingx: свободно 5 USDT, кандидату нужно "),
        "{t}"
    );
    assert!(
        t.ends_with("на свободный остаток 5 сделка невыгодна: прибыль ниже порога"),
        "{t}"
    );
    // Списание кандидата больше предела расчёта — быстрый путь не применяется (сразу полный подбор под предел).
    let cx = cx(&rec);
    let mut w = Want {
        spend: Some(d("25")),
        ..Want::new(d("10"))
    };
    let got = Box::pin(recalc(
        &core,
        &cx,
        &mut w,
        d("20"),
        rec.pair.step_size,
        (d("0.1"), d("0.1")),
    ))
    .await;
    assert!(got.is_none());
    assert!(
        matches!(&w.path, Some(Path::Off(t)) if t.contains("больше предела списания 20")),
        "{:?}",
        w.path
    );
    // Списание в пределе — проверка не мешает: быстрый путь идёт к расчёту (здесь он отказывает: площадки не подключены).
    let mut w = Want {
        spend: Some(d("20")),
        ..Want::new(d("10"))
    };
    let _ = Box::pin(recalc(
        &core,
        &cx,
        &mut w,
        d("20"),
        rec.pair.step_size,
        (d("0.1"), d("0.1")),
    ))
    .await;
    assert!(matches!(&w.path, Some(Path::Full(_))), "{:?}", w.path);
}

#[tokio::test]
async fn every_switch_combination_keeps_first_reason() {
    let (core, _st) = core_at("fast-266-combo").await;
    let rec = Rec::load("fast-266-combo").await;
    let cand = Cand::of(&size_common::calc::calc(&rec, d("10")));
    let all = Opts {
        qty: Some(cand.qty),
        parallel: true,
        networks_memory: true,
        balance: Some(cand),
    };
    let off = Opts::default();
    let sets = [
        off,
        all,
        Opts {
            qty: all.qty,
            ..off
        },
        Opts {
            parallel: true,
            ..off
        },
        Opts {
            networks_memory: true,
            ..off
        },
        Opts {
            balance: all.balance,
            ..off
        },
        Opts {
            parallel: true,
            balance: all.balance,
            ..off
        },
        Opts {
            qty: all.qty,
            balance: all.balance,
            ..off
        },
    ];
    for (buy, sell) in [
        ("bingx", "bsc_dex"),
        ("bsc_dex", "bingx"),
        ("bingx", "mexc"),
    ] {
        let dd = dir_of(buy, sell);
        let base = Box::pin(precheck::run(&core, &dd, false, off, None))
            .await
            .unwrap_err();
        for o in sets {
            let e = Box::pin(precheck::run(&core, &dd, true, o, None))
                .await
                .unwrap_err();
            assert_eq!(reason(&base), reason(&e), "{buy} → {sell}: {o:?}");
        }
    }
}
