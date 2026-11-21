//! Файл: tests/core_coin_reentry_t179.rs
//! Что делает: решение 394 (t179) — докупка (записка договорённостей 269) на настоящем ядре и SQLite во временной папке
//! (площадок в реестре нет, моков нет):
//! - п. 15: exec.top_up не задана — одна сделка на монету, отказ прежним текстом (start_reason::busy), у сделки нет
//!   группы (JSON сделки без поля group), после удачной сделки пауза направления ставится как раньше (попытка паузы —
//!   Warn core.ledger о signal.route_cooldown_s), строки «Пауза не поставлена» нет; событие баланса без хеша при двух
//!   ожиданиях закрывает подходящее по сумме, как раньше;
//! - п. 1, 3: включена — вторая сделка ТОГО ЖЕ направления присоединяется к группе первой, когда первая учла покупку;
//!   застрявшая сделка группы (во всех направлениях), замок целиком — прежний отказ; ручное действие — отказ;
//! - решение 405 (t191): другие направления монеты при идущей группе — своя группа (Lead / Join своего направления),
//!   лимит монеты общий, продажа после монет всех групп на той же площадке, общий возврат группы не ждёт чужую группу;
//!   выключено — другие направления прежний отказ; встречная сделка на общем счёте (тот же кошелёк DEX по маппингу
//!   или та же биржа) — отказ своей причиной; да → нет при идущих группах — прежний отказ, котировки как без докупки;
//!   резерв возвратов USDT (usdt_hold.rs: отложенные части группы, «идёт», «ждёт повтора»; по площадке, у DEX — по
//!   кошельку; цепочка limits::cap_of — та же сумма в вычете и в тексте); сверка остатка без чужих монет на общем
//!   счёте (remain_others.rs: общая площадка покупки, кошелёк DEX, перевод в площадку; «неясные» не отключают вычет
//!   доказуемых — ревью 4); принятый площадкой вывод снимает резерв части сразу и после перезапуска (usdt_accepted.rs,
//!   ревью 4 Б2, ревью 5 Н1);
//! - деньги: бюджет докупки = лимит монеты − вложенное идущих (limits_parts.rs);
//! - п. 4: продажа докупки — разность котировок на записанных bids BingX CAKE-USDT (gateway::frozen::FrozenBook::price)
//!   и пуле CAKE/USDT V2 (математика адаптера) против последовательной продажи; покупка — данные новее покупки идущей
//!   (стакан / блок; агрегатор без маршрута — отказ);
//! - след продажи (ride_trail.rs): стакан не новее продажи сделки группы — отказ, новее — след снят;
//! - пп. 8, Б2: включена — событие без хеша при двух ожиданиях ничего не закрывает (сделка 1 ждёт 100, сделка 2 — 95, у
//!   первой дошло 95), только будит опрос;
//! - пп. 10, 11, 13: общий возврат USDT группы — первая закрытая ждёт (возврат 0), последняя перегоняет общую сумму,
//!   у отложенной — «общий возврат в сделке …»; последняя идущая застряла — отложенные уходят одним возвратом сразу;
//! - пауза после удачной сделки группы (route_pause.rs won);
//! - ревью 3: общий возврат кусками выше максимума вывода (максимум и комиссия USDT BEP20 — запись BingX
//!   config/getall 24.10, разбор адаптера) — 2 и 3 куска дошли, средний отказал (точный остаток), перезапуск посреди
//!   кусков (ledger::load); падение после сохранения носителя до вложенной (Н2); вложенная в группе с admit (Н1);
//!   после общего возврата новая сделка — новая группа, ушедшая с пути — «была частью группы» (Н3, п. 13); «Итог
//!   группы» один раз у уже известившего носителя и текст вышедшей с чужой выручкой (Н4);
//! - ревью 4: застрявшая и проданная на B копией задачи без пометки — свой возврат (Н7, настоящий
//!   usdt_return_deal::prepare); «Перегнать» остатка кусками — учёт продолжается от дошедших (Н8); ни один кусок не
//!   больше максимума (Н10); газ неудачных перегонов USDT — сделка по ID перегона «u» / «up<n>» / «m…» (п. 14).
//!
//! Ставка комиссии тейкера в проверке разности — параметр проверки (в записях BingX ставки аккаунта нет,
//! tests/size_common/mod.rs), не данные биржи. Не покрыто здесь (нужны живые площадки): путь executor::accepted → board
//! → release целиком, Live-источник расчёта с котировками шлюза, симуляция продажи DEX, сам перегон USDT.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::venues::bsc_dex (математика
//! пула), tests/size_common.

mod size_common;

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use alloy_primitives::{Address, B256};
use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::coin_ride::Seat;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::deal_return_cost::Relayed;
use arb::core::deps::Deps;
use arb::core::dry::Mode;
use arb::core::types::{
    Asset, BalanceChange, BalanceKind, ChainTx, Cost, CostKind, DexRoute, OrderBook, PoolKind,
    PoolRef, PoolState, PriceLevel, Quote, RouteHop, RouteLeg, RouteMode, SendMethod, Side,
    TradeRequest, TradeResult, TradeStatus, TxOutcome, VenueId,
};
use arb::core::usdt_return_state::UsdtReturn;
use arb::core::{
    coin_group, coin_ride, credit_event, credit_watch, deal_return_cost, group_notice,
    limits_parts, manual_usdt, relay_owner, ride_quote, ride_trail, route_pause, start_reason,
    transfer_credit, usdt_hold, usdt_return_deal, usdt_return_parts, usdt_return_pieces,
};
use arb::gateway::contract::VenueKind;
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use size_common::{CAKE, PAIR, Rec, d};
use tokio::sync::broadcast;

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-t179-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке; площадки bingx, mexc — биржи, bsc_dex — DEX (вид площадки для проверок свежести).
/// Возвращает ядро, настройки, журнал.
async fn start(name: &str) -> (Core, Store, Journal) {
    let d = dir(name);
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    (core, settings, journal)
}

/// Задать настройку всей системы. Ошибок нет (тест падает).
async fn set(s: &Store, name: &str, v: serde_json::Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Направление CAKE (записи tests/size_common): A → B.
fn dir_of(a: &str, b: &str) -> arb::core::types::Direction {
    let mut x = size_common::calc::direction();
    x.buy_on = VenueId(a.into());
    x.sell_on = VenueId(b.into());
    x
}

/// Сделка, учётшая покупку: шаг, вложено, монет; покупка — запрос с маршрутом и транзакцией (у биржи — None); группа
/// докупки (None — без докупки).
fn bought(
    id: &str,
    dir: &arb::core::types::Direction,
    step: Step,
    (invested, qty): (&str, &str),
    group: Option<&str>,
) -> Deal {
    let mut x = Deal::new(id.into(), dir.clone(), json!({}), d("1"));
    x.group = group.map(|g| Box::new(DealGroup::new(g)));
    x.step = step;
    x.invested = d(invested);
    x.qty = d(qty);
    x.buy = Some(trade(id, dir, None, None, Utc::now()));
    x
}

/// Исполнение покупки (статус Filled) с маршрутом DEX и блоком транзакции (None — биржа).
fn trade(
    id: &str,
    dir: &arb::core::types::Direction,
    route: Option<DexRoute>,
    block: Option<u64>,
    at: chrono::DateTime<Utc>,
) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: dir.buy_on.clone(),
            asset: dir.asset.clone(),
            side: Side::Buy,
            qty: d("10"),
            worst_price: d("3"),
            deal_id: id.into(),
            route,
            exit_position: false,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: d("10"),
        avg_price: d("2"),
        fees: vec![],
        venue_ref: id.into(),
        chain: block.map(|b| ChainTx {
            hash: B256::ZERO,
            nonce: 0,
            channels: vec![],
            method: SendMethod::Single,
            outcome: TxOutcome::Included,
            block: Some(b),
            gas_used: None,
            gas_bnb: None,
            tip_bnb: None,
            cancel_hash: None,
            cancel_gas_bnb: None,
            approve_gas_bnb: None,
            tip_hash: None,
            tip_gas_bnb: None,
        }),
        finished_at: at,
        executor: None,
    }
}

/// Сохранить сделку в учёт и занять монету так, как это делает исполнитель (admit → board). Возвращает место.
async fn ride(core: &Core, deal: &Deal) -> Seat {
    let seat = coin_ride::admit(core, &deal.direction).unwrap();
    arb::core::ledger::save(core, deal).await;
    let g = deal.group.as_ref().map(|g| g.id.clone());
    coin_ride::board(core, &deal.direction, &deal.id, g);
    seat
}

/// Включить докупку (exec.top_up = да). Ошибок нет (тест падает).
async fn top_up(s: &Store) {
    set(s, coin_ride::TOP_UP, json!(true)).await;
}

#[tokio::test]
async fn off_keeps_one_deal_per_coin_with_old_text() {
    let (core, _s, _j) = start("off").await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    // До покупки — замок целиком: второй вход отказан, ручное действие тоже.
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Alone));
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(!coin_ride::take(&core, &key));
    coin_ride::release(&core, &key, None);
    assert!(!coin_ride::held(&core, &key));
    // Покупка учтена, сделка везёт монету: одна сделка на монету, отказ прежним текстом.
    let first = bought("D1", &dx, Step::Transfer, ("40", "10"), None);
    assert_eq!(ride(&core, &first).await, Seat::Alone);
    let why = coin_ride::admit(&core, &dx).unwrap_err();
    let old = start_reason::busy(core.inner.deals.lock().unwrap().values(), &dx);
    assert_eq!(why, old);
    assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
    assert!(coin_ride::blocks_new(&core, &dx));
    assert!(
        !coin_ride::take(&core, &key),
        "ручное действие при идущей — отказ"
    );
    // Сделка без группы — в JSON поля group нет (запись и API как раньше).
    let v = serde_json::to_value(&first).unwrap();
    assert!(v.get("group").is_none());
    coin_ride::release(&core, &key, Some("D1"));
    assert!(!coin_ride::held(&core, &key));
    assert!(coin_ride::admit(&core, &dx).is_ok());
}

#[tokio::test]
async fn off_successful_close_still_pauses_direction() {
    let (core, _s, j) = start("off-pause").await;
    let dx = dir_of("bingx", "bsc_dex");
    for (id, group) in [("P1", None), ("P2", Some("P2"))] {
        let mut x = bought(id, &dx, Step::Selling, ("40", "0"), group);
        x.at = dx.sell_on.clone();
        x.proceeds = d("41");
        x.sells.push(trade(id, &dx, None, None, Utc::now()));
        arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано").await;
        let won_line = x.timeline.iter().any(|(_, t)| t == route_pause::WON_NOTE);
        let warned = j.list(500, None).await.unwrap().iter().any(|e| {
            e.kind == "core.ledger"
                && e.level == Level::Warn
                && e.message.contains("signal.route_cooldown_s")
        });
        if group.is_none() {
            // Без группы (докупка выключена): попытка паузы, как раньше (signal.route_cooldown_s не задана — Warn).
            assert!(!won_line);
            assert!(warned, "пауза после удачной сделки — как раньше");
        } else {
            assert!(won_line, "у удачной сделки группы паузы нет");
        }
    }
}

#[tokio::test]
async fn same_direction_joins_other_directions_own_group_stuck_holds_all() {
    let (core, s, _j) = start("join").await;
    top_up(&s).await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1"));
    assert_eq!(ride(&core, &first).await, Seat::Lead);
    // Тот же направление — докупка в группу D1; до её покупки монета занята целиком.
    assert!(!coin_ride::blocks_new(&core, &dx));
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Join("D1".into())));
    assert!(coin_ride::admit(&core, &dx).is_err());
    coin_ride::release(&core, &key, None);
    // Решение 405 (t191, ответ Тиграна 18.11): другие направления монеты при идущей группе — своя новая группа (Lead).
    for other in [dir_of("mexc", "bsc_dex"), dir_of("bingx", "mexc")] {
        assert!(!coin_ride::blocks_new(&core, &other));
        assert_eq!(coin_ride::admit(&core, &other), Ok(Seat::Lead));
        coin_ride::release(&core, &key, None);
    }
    // Встречное (покупает на DEX, где D1 продаёт; кошелёк не задан — считается тем же) — отказ своей причиной (Н1).
    let back = dir_of("bsc_dex", "bingx");
    assert!(!coin_ride::blocks_new(&core, &back), "причина — у admit");
    let why = coin_ride::admit(&core, &back).unwrap_err();
    assert!(
        why.starts_with(
            "новая сделка CAKE DEX → BingX не начата: идёт встречная сделка D1 (CAKE BingX → DEX, шаг «перевод», \
             10 CAKE на BingX)"
        ),
        "{why}"
    );
    assert!(
        !coin_ride::take(&core, &key),
        "ручное действие при идущей — отказ"
    );
    // Сделка группы застряла — новых сделок монеты нет ни в одном направлении (решение 405 — как у докупки).
    first.step = Step::Stuck;
    arb::core::ledger::save(&core, &first).await;
    assert!(coin_ride::admit(&core, &dx).is_err());
    assert!(coin_ride::blocks_new(&core, &dx));
    let why = coin_ride::admit(&core, &dir_of("mexc", "bsc_dex")).unwrap_err();
    assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
    assert!(coin_ride::blocks_new(&core, &dir_of("mexc", "bsc_dex")));
    // Сделка без группы (начата до включения) — докупок нет.
    coin_ride::release(&core, &key, Some("D1"));
    let old = bought("D0", &dx, Step::Transfer, ("40", "10"), None);
    arb::core::ledger::save(&core, &old).await;
    coin_ride::board(&core, &dx, "D0", None);
    assert!(coin_ride::admit(&core, &dx).is_err());
}

/// Решение 405 (t191): при идущей группе монеты сделка ДРУГОГО направления — своя группа; докупки каждого направления —
/// в свою группу; лимит монеты — общий на все направления; продажа — после монет всех групп на той же площадке;
/// общий возврат одной группы не ждёт сделок другой. Выключено — другие направления отказ прежним текстом.
#[tokio::test]
async fn other_directions_trade_in_own_groups() {
    let (core, s, _j) = start("other-dirs").await;
    set(&s, "money.max_per_trade_usd", json!("50")).await;
    set(&s, "money.max_per_coin_usd", json!("70")).await;
    set(&s, "money.max_in_transit_usd", json!("100")).await;
    let (dx, ox) = (dir_of("bingx", "bsc_dex"), dir_of("mexc", "bsc_dex"));
    let key = CoinKey::of(&dx.asset);
    // Выключено (п. 15): идёт сделка без группы — другое направление прежний отказ.
    ride(&core, &bought("D0", &dx, Step::Transfer, ("10", "1"), None)).await;
    let why = coin_ride::admit(&core, &ox).unwrap_err();
    assert!(why.starts_with("по монете идёт другая сделка D0"), "{why}");
    coin_ride::release(&core, &key, Some("D0"));
    let mut d0 = bought("D0", &dx, Step::Closed, ("10", "0"), None);
    d0.result_usd = Some(d("0"));
    arb::core::ledger::save(&core, &d0).await;
    top_up(&s).await;
    assert_eq!(
        ride(
            &core,
            &bought("D1", &dx, Step::Selling, ("40", "10"), Some("D1"))
        )
        .await,
        Seat::Lead
    );
    // Другое направление — первая своей группы; план возврата — с фиксированной частью (группы направления нет).
    assert_eq!(coin_ride::group_running(&core, &ox), None);
    assert_eq!(
        ride(
            &core,
            &bought("O1", &ox, Step::Transfer, ("20", "5"), Some("O1"))
        )
        .await,
        Seat::Lead
    );
    // Докупки — каждая в группу своего направления (раньше group_running смотрел только первую запись монеты).
    assert_eq!(coin_ride::group_running(&core, &dx).as_deref(), Some("D1"));
    assert_eq!(coin_ride::group_running(&core, &ox).as_deref(), Some("O1"));
    assert_eq!(coin_ride::admit(&core, &ox), Ok(Seat::Join("O1".into())));
    coin_ride::release(&core, &key, None);
    coin_group::dropped(&core, &key, "O1");
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Join("D1".into())));
    coin_ride::release(&core, &key, None);
    coin_group::dropped(&core, &key, "D1");
    // Лимит монеты — общий: 70 − 40 (D1) − 20 (O1).
    let p = limits_parts::parts(&core, &dir_of("bsc_dex", "bingx"), &Mode::Live).unwrap();
    assert_eq!(p.coin_left, d("10"));
    // Продажа на bsc_dex — после монет обеих групп (10 + 5); на bingx никто не продаёт.
    assert_eq!(ride_quote::sell_ahead(&core, &dx), d("15"));
    assert_eq!(
        ride_quote::sell_ahead(&core, &dir_of("bsc_dex", "bingx")),
        d("0")
    );
    // Общий возврат D1 уходит, когда закрылась последняя сделка ЕЁ группы, — O1 (другая группа) не ждётся.
    let mut x = sold("D1", &dx, "D1");
    assert!(close(&core, &mut x, "41").await.is_empty());
    wait_note(&core, "D1", "общий возврат USDT группы D1: 41").await;
    assert!(!coin_group::flushed(&core, "O1"));
    assert!(!has_note(&core, "O1", "общий возврат"));
}

/// Кошелёк DEX в маппинге (решение 132): добавить и привязать к строке биржи. Возвращает id кошелька.
async fn wallet(s: &Store, row: &str, name: &str) -> String {
    let a = s.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(
        &id,
        name,
        &format!("wallet.{id}.key"),
        &format!("wallet:{id}"),
        "тест",
    )
    .await
    .unwrap();
    a.map_set(row, Some(&id), "тест").await.unwrap();
    id
}

/// Решение 405, ответ Тиграна на Н1: встречная сделка на общем счёте не входит, пока идёт другая. Общий счёт — тот же
/// кошелёк DEX (маппинг) или та же биржа; другой кошелёк — можно.
#[tokio::test]
async fn counter_direction_on_same_wallet_waits() {
    let (core, s, _j) = start("counter").await;
    top_up(&s).await;
    let wb = wallet(&s, "bingx", "BingX").await;
    let wm = wallet(&s, "mexc", "MEXC").await;
    let dx = dir_of("bingx", "bsc_dex");
    let key = CoinKey::of(&dx.asset);
    let mut d1 = bought("D1", &dx, Step::Selling, ("40", "10"), Some("D1"));
    d1.dex_account = Some(arb::core::types::DexAccountId(wb.clone()));
    assert_eq!(ride(&core, &d1).await, Seat::Lead);
    // DEX → BingX — покупка на том же кошельке W BingX, где D1 продаёт: отказ с причиной (журнал core.start_refused).
    let why = coin_ride::admit(&core, &dir_of("bsc_dex", "bingx")).unwrap_err();
    // Требование Тиграна 18.11: какая новая, какая встречная (ID, откуда → куда, шаг, монеты и где), какой счёт общий.
    let want = format!(
        "новая сделка CAKE DEX → BingX не начата: идёт встречная сделка D1 (CAKE BingX → DEX, шаг «продажа», 10 CAKE \
         на BingX). Новая купила бы CAKE на DEX, где встречная его продаёт, и у них один кошелёк {wb} на DEX — монеты \
         двух сделок легли бы вместе"
    );
    assert!(why.starts_with(&want), "{why}");
    assert!(
        why.contains("когда сделка D1 закончится, это направление снова торгуется"),
        "{why}"
    );
    // MEXC → BingX — продажа на BingX, где D1 купила (один счёт биржи): отказ.
    let why = coin_ride::admit(&core, &dir_of("mexc", "bingx")).unwrap_err();
    assert!(
        why.contains("Новая продала бы CAKE на BingX, где встречная его купила, и у них один счёт на бирже BingX"),
        "{why}"
    );
    // Другой кошелёк (DEX → MEXC — кошелёк MEXC) и не встречные (MEXC → DEX, BingX → MEXC) — можно.
    assert_ne!(wb, wm);
    for ok in [
        dir_of("bsc_dex", "mexc"),
        dir_of("mexc", "bsc_dex"),
        dir_of("bingx", "mexc"),
    ] {
        assert_eq!(coin_ride::admit(&core, &ok), Ok(Seat::Lead), "{ok:?}");
        coin_ride::release(&core, &key, None);
    }
    // D1 кончилась — встречное входит.
    coin_ride::release(&core, &key, Some("D1"));
    assert_eq!(
        coin_ride::admit(&core, &dir_of("bsc_dex", "bingx")),
        Ok(Seat::Lead)
    );
}

/// М3: докупку выключили (да → нет) при идущих группах — другие направления снова «занято» прежним текстом, котировки
/// и зачисление — как без докупки; идущие группы доводятся.
#[tokio::test]
async fn switch_off_with_other_direction_groups_running() {
    let (core, s, _j) = start("switch-off").await;
    top_up(&s).await;
    let (dx, ox) = (dir_of("bingx", "bsc_dex"), dir_of("mexc", "bsc_dex"));
    ride(
        &core,
        &bought("D1", &dx, Step::Selling, ("40", "10"), Some("D1")),
    )
    .await;
    ride(
        &core,
        &bought("O1", &ox, Step::Transfer, ("20", "5"), Some("O1")),
    )
    .await;
    set(&s, coin_ride::TOP_UP, json!(false)).await;
    for dd in [&dx, &ox, &dir_of("bingx", "mexc")] {
        let why = coin_ride::admit(&core, dd).unwrap_err();
        assert!(why.starts_with("по монете идёт другая сделка"), "{why}");
        assert!(coin_ride::blocks_new(&core, dd));
        assert!(ride_quote::idle(&core, dd), "котировки — как без докупки");
    }
    // Группы остаются: общий возврат D1 по её закрытию, O1 не ждётся.
    let mut x = sold("D1", &dx, "D1");
    assert!(close(&core, &mut x, "41").await.is_empty());
    wait_note(&core, "D1", "общий возврат USDT группы D1: 41").await;
}

/// Решение 405 (ответ Тиграна «резервировать»): USDT, ждущие общего возврата группы, покупка не тратит — резерв по
/// площадке (у DEX — по кошельку сделки); общий возврат ушёл — резерва нет.
#[tokio::test]
async fn waiting_group_usdt_reserved_from_buys() {
    let (core, s, _j) = start("hold").await;
    top_up(&s).await;
    let bingx = VenueId("bingx".into());
    assert_eq!(
        usdt_hold::held(&core, &bingx, None),
        usdt_hold::Held::default()
    );
    // Группа G MEXC → BingX: D2 закрылась первой — 95 USDT ждут на BingX общего возврата.
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    let mut x = sold("D2", &dx, "G");
    assert!(close(&core, &mut x, "95").await.is_empty());
    let h = usdt_hold::held(&core, &bingx, None);
    assert_eq!(h.amount, d("95"));
    assert_eq!(
        h.deals,
        vec![(
            "D2".to_string(),
            "CAKE MEXC → BingX: выручка ждёт общего возврата группы G на MEXC".to_string()
        )]
    );
    // М1: докупку выключили (да → нет) — отложенная часть идущей группы остаётся в резерве до общего возврата.
    set(&s, coin_ride::TOP_UP, json!(false)).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None), h);
    // М6 / М7: цепочка buy_free → buy_cap (limits::cap_of): свободно 120 — после резерва 25, бюджет 50 — предел 25;
    // свободно 60 — 0 и отказ с ТОЙ ЖЕ суммой резерва, что вычтена (начало — none_free: метка «баланс»).
    let quote = "USDT".to_string();
    let cap = arb::core::limits::cap_of(
        &bingx,
        d("50"),
        (usdt_hold::spendable(d("120"), &h), quote.clone()),
        &h,
    );
    assert_eq!(cap, Ok((d("25"), d("25"), quote.clone())));
    // Как его собирает buy_free: прочитанное свободно и площадка (требование Тиграна 18.11 — что хотели, сколько на
    // счёте, сколько и чьё в резерве, что делать).
    let mut hr = h.clone();
    hr.free = Some(d("60"));
    hr.place = Some("BingX".into());
    let text = arb::core::limits::cap_of(
        &bingx,
        d("50"),
        (usdt_hold::spendable(d("60"), &hr), quote.clone()),
        &hr,
    )
    .unwrap_err();
    assert_eq!(
        text,
        arb::core::limits::zero_text(&bingx, "USDT", &hr, Some(d("50")))
    );
    assert_eq!(
        text,
        "на bingx свободно 0 USDT для покупки, хотели купить на 50.00 USDT; на BingX сейчас 60.00 USDT — ещё 95.00 \
         USDT в резерве возвратов USDT: сделка D2 (CAKE MEXC → BingX: выручка ждёт общего возврата группы G на MEXC) — 95.00 USDT; эти USDT ждут возврата, бот их не трогает \
         (решение 405). Покупку не делаем. Что сделать: дождитесь этих возвратов или разберите «не вернулись» на \
         экране «Ручное управление» → «Резерв USDT возвратов»"
    );
    let back = dir_of("bingx", "mexc");
    assert!(arb::core::precheck_balance::is_balance(&back, &text));
    // На DEX — только тот же кошелёк.
    let dd = dir_of("bingx", "bsc_dex");
    let mut y = bought("E1", &dd, Step::Closed, ("40", "0"), Some("E"));
    y.group.as_mut().unwrap().wait = Some((dd.sell_on.clone(), d("30")));
    y.dex_account = Some(arb::core::types::DexAccountId("w1".into()));
    arb::core::ledger::save(&core, &y).await;
    let w1 = arb::core::types::DexAccountId("w1".into());
    let w2 = arb::core::types::DexAccountId("w2".into());
    assert_eq!(
        usdt_hold::held(&core, &dd.sell_on, Some(&w1)).amount,
        d("30")
    );
    assert_eq!(
        usdt_hold::held(&core, &dd.sell_on, Some(&w2)).amount,
        Decimal::ZERO
    );
    // Общий возврат G начат (последняя G закрылась) — отложенных частей нет; в резерве — только сам перегон, пока он
    // «идёт» / «ждёт повтора» (без площадок он кончается «не вернулись» — не в резерве).
    let mut g = sold("G", &dx, "G");
    assert!(close(&core, &mut g, "100").await.is_empty());
    wait_note(&core, "G", "общий возврат USDT группы G: 195").await;
    let after = usdt_hold::held(&core, &bingx, None);
    assert!(
        after
            .deals
            .iter()
            .all(|(_, w)| !w.contains("ждёт общего возврата группы")),
        "{after:?}"
    );
}

/// Решение 405 (Тигран 18.11: резервировать и начатые возвраты, и «не вернулись»): части «идёт», «ждёт повтора» и
/// «не вернулись» (общего и одиночного возврата) — в резерве площадки; «вернулись» — нет. Ревью 3: Н2 — у «идёт»
/// ушедший вывод (хеш вывода в ожидании зачисления перегона) из резерва вычитается; Н3 — перегон своей сделки видит
/// резерв только других; М2 — резерв считается по набору unsettled (закрытые без возврата в нём не держатся).
#[tokio::test]
async fn started_returns_reserved_until_done() {
    let (core, _s, _j) = start("hold-parts").await;
    let (bingx, mexc) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let dx = dir_of("mexc", "bingx");
    let mut x = bought("S1", &dx, Step::Closed, ("40", "0"), None);
    x.usdt_return = Some(UsdtReturn::Pending {
        amount: d("41"),
        from: bingx.clone(),
        to: mexc.clone(),
    });
    arb::core::ledger::save(&core, &x).await;
    // Решение 227: у сделки одна часть на площадку — «не вернулись» на BingX у другой сделки.
    let mut n = bought("S4", &dx, Step::Closed, ("40", "0"), None);
    n.usdt_return = Some(UsdtReturn::NotReturned {
        amount: d("7"),
        at: bingx.clone(),
        why: "тест".into(),
    });
    arb::core::ledger::save(&core, &n).await;
    let mut y = bought("S2", &dx, Step::Closed, ("40", "0"), None);
    y.usdt_return = Some(UsdtReturn::Retrying {
        amount: d("30"),
        from: bingx.clone(),
        to: mexc.clone(),
        tries: 1,
        why: "тест".into(),
    });
    arb::core::ledger::save(&core, &y).await;
    // Закрытая без возврата — не в наборе unsettled (М2).
    let z = bought("S3", &dx, Step::Closed, ("40", "0"), None);
    arb::core::ledger::save(&core, &z).await;
    assert!(!core.inner.unsettled.lock().unwrap().contains("S3"));
    let h = usdt_hold::held(&core, &bingx, None);
    assert_eq!(h.amount, d("78"));
    assert_eq!(
        h.deals,
        vec![
            (
                "S1".to_string(),
                "CAKE MEXC → BingX: перегон выручки на MEXC идёт".to_string()
            ),
            (
                "S2".to_string(),
                "CAKE MEXC → BingX: перегон выручки на MEXC ждёт повтора (тест)".to_string()
            ),
            (
                "S4".to_string(),
                "CAKE MEXC → BingX: выручка не вернулась на MEXC (тест) — ждёт кнопки «Перегнать» или \
                 «Перегнано вручную»"
                    .to_string()
            ),
        ]
    );
    assert_eq!(usdt_hold::held(&core, &mexc, None).amount, Decimal::ZERO);
    // Н3: перегон S1 («S1u2» — повтор 2) видит резерв только других (S2: 30, S4: 7), свой — тратит.
    assert_eq!(usdt_hold::owner_of(&core, "S1u2").as_deref(), Some("S1"));
    assert_eq!(usdt_hold::owner_of(&core, "tu20261118"), None);
    let others = usdt_hold::held_except(&core, &bingx, None, Some("S1"));
    assert_eq!(others.amount, d("37"));
    // Н2: вывод перегона S1 ушёл (хеш от отправителя в ожидании зачисления 40.9) — из «идёт» 41 вычитается приход.
    let t = credit_watch::register(&core, "S1u", &bingx, &mexc, &usdt(), d("40.9"));
    assert_eq!(
        usdt_hold::held(&core, &bingx, None).amount,
        d("78"),
        "хеша ещё нет"
    );
    t.set_tx("0xabc");
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("37.1"));
    // Перевод монеты самой сделки (ID без суффикса перегона) — не перегон USDT.
    let c = credit_watch::register(&core, "S2", &bingx, &mexc, &usdt(), d("30"));
    c.set_tx("0xdef");
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("37.1"));
    drop((t, c));
    // Вернулись — резерва S1 нет, S1 больше ничего не держит — из набора уходит; «не вернулись» S4 — в резерве.
    x.usdt_return = Some(UsdtReturn::Done {
        amount: d("41"),
        credited: d("40"),
        from: bingx.clone(),
        to: mexc.clone(),
        sent: None,
        fee_usd: None,
    });
    arb::core::ledger::save(&core, &x).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("37"));
    // «Перегнано вручную» по «не вернулись» (кнопка человека) — резерва S4 нет.
    n.usdt_return = Some(UsdtReturn::Manual {
        amount: d("7"),
        at: bingx.clone(),
        to: mexc,
        by: "тест".into(),
        credited: None,
    });
    arb::core::ledger::save(&core, &n).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("30"));
    assert!(!core.inner.unsettled.lock().unwrap().contains("S1"));
}

/// Решение 405 (Тигран 18.11: «каждая сделка знает своё количество»; ревью Н1′ / Н7, ревью 3 Б1, Н1, М3, М5): сверка
/// остатка (remain_fact::apply_fact — после чтения факта) не отдаёт сделке монеты других сделок, доказуемо лежащих на
/// общем счёте; сделка в пути — «неясно» (других не вычитаем, алерта нет); «Беру на себя» — не другие; остановленная
/// с монетами (в том числе фантом PIT) — другие.
#[tokio::test]
async fn remain_fact_keeps_only_own_coins_on_shared_account() {
    use arb::core::remain_fact::apply_fact;
    use arb::core::remain_others::{Own, on_account, own_qty};
    let (core, _s, j) = start("own-coins").await;
    let bingx = VenueId("bingx".into());
    let dx = dir_of("bingx", "bsc_dex");
    let w = |id: &str| Some(arb::core::types::DexAccountId(id.into()));
    // Б1 (а): A перевела 10 с BingX (шаг «перевод» — где её монеты, не доказано), B застряла на BingX: учёт 5, факт 5.
    // Решение 413 (повторное ревью t198, С-1): A УХОДИТ с BingX (получатель — DEX) — не «в пути сюда», а «уходит
    // отсюда» (Others.leaving); как и раньше, не вычитается и алерта нет — меняется только раскладка (замер удержания и
    // снимок посторонних при ней неоднозначны).
    let a = bought("A", &dx, Step::Transfer, ("40", "10"), Some("A"));
    arb::core::ledger::save(&core, &a).await;
    let mut b = bought("B", &dx, Step::Stuck, ("20", "5"), Some("A"));
    arb::core::ledger::save(&core, &b).await;
    let o = on_account(&core, &b, &bingx);
    assert!(o.unclear.is_empty());
    assert_eq!(o.leaving, vec!["A".to_string()]);
    assert_eq!(o.qty, Decimal::ZERO);
    assert_eq!(own_qty(d("5"), d("5"), &o), Own::Qty(d("5")));
    assert!(
        apply_fact(&core, &mut b, d("5")).await,
        "ни урезания, ни алерта"
    );
    assert_eq!(b.qty, d("5"));
    // «Неясно» и факт меньше учёта — решение 222 как раньше: min(учёт, факт).
    assert!(apply_fact(&core, &mut b, d("4")).await);
    assert_eq!(b.qty, d("4"));
    let saved = core.inner.deals.lock().unwrap().get("B").cloned().unwrap();
    assert_eq!(saved.qty, d("4"));
    // Общая площадка покупки, монеты доказуемо там: A застряла на BingX (не перевод) с 10 → B на 12 ведёт 2.
    let mut a = a;
    a.step = Step::Stuck;
    arb::core::ledger::save(&core, &a).await;
    b.qty = d("5");
    let o = on_account(&core, &b, &bingx);
    assert_eq!(o.deals, vec![("A".to_string(), d("10"))]);
    assert!(apply_fact(&core, &mut b, d("12")).await);
    assert_eq!(b.qty, d("2"));
    // Требование Тиграна 18.11: текст — какая сделка, сколько по учёту / на счёте / у других (кто), сколько продаём и
    // сколько не хватает, что проверить.
    let rows = j.list(500, None).await.unwrap();
    let msg = rows
        .iter()
        .find(|e| e.kind == "core.remain_fact" && e.message.contains("из них 10 CAKE"))
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        msg.starts_with(
            "сделка B (CAKE BingX → DEX, шаг «монета застряла», 5 CAKE на BingX): по учёту у сделки 5 CAKE, а на \
             счёте BingX сейчас 12 CAKE, из них 10 CAKE принадлежат другим сделкам: сделка A (CAKE BingX → DEX, \
             шаг «монета застряла», 10 CAKE на BingX). Бот продаёт только монеты этой сделки: 2 CAKE; не хватает \
             3 CAKE — проверьте баланс CAKE на BingX и хронологию сделки B"
        ),
        "{msg}"
    );
    // Пример записки: учёт 1000, кошелёк 900, другие 500 → 400; факт 0 → закрыть; других нет — как settle_qty.
    let others = |q: &str| arb::core::remain_others::Others {
        qty: d(q),
        deals: vec![],
        unclear: vec![],
        // Решение 413: ждущие доли других — в отдельном поле (здесь нет).
        pending: vec![],
        leaving: vec![],
        leaving_qty: Decimal::ZERO,
        rests: vec![],
        words: vec![],
    };
    assert_eq!(
        own_qty(d("1000"), d("900"), &others("500")),
        Own::Qty(d("400"))
    );
    assert_eq!(
        own_qty(d("500"), Decimal::ZERO, &others("1000")),
        Own::Qty(Decimal::ZERO)
    );
    assert_eq!(
        own_qty(d("100"), d("150"), &others("0")),
        Own::Qty(arb::core::remain_fact::settle_qty(d("100"), d("150")))
    );
    // Н1: «Беру на себя» с 100 на BingX — не другие (монеты у человека); остановленная с фантомом PIT — другие: факт
    // 19259.884524163 (запись живого случая 08.11), её учёт 12162067866.1632 → B не закрывается, алерт человеку.
    let mut t = bought("T", &dx, Step::TakenOver, ("40", "100"), None);
    t.at = bingx.clone();
    arb::core::ledger::save(&core, &t).await;
    let mut h = bought("H", &dx, Step::Halted, ("40", "12162067866.1632"), None);
    h.at = bingx.clone();
    arb::core::ledger::save(&core, &h).await;
    a.step = Step::Closed;
    arb::core::ledger::save(&core, &a).await;
    let o = on_account(&core, &b, &bingx);
    assert_eq!(o.deals, vec![("H".to_string(), d("12162067866.1632"))]);
    assert!(!apply_fact(&core, &mut b, d("19259.884524163")).await);
    let rec = core.inner.deals.lock().unwrap().get("B").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman, "алерт — решает человек");
    assert_eq!(rec.qty, d("2"), "учёт не тронут");
    // Б1 (б), кошелёк DEX: W1 застряла на DEX (учёт 5, факт 5), W5 идёт на тот же кошелёк (2 ещё не дошли) — «неясно»,
    // 5 остаётся; продающая там W2 того же кошелька — другие; другой кошелёк и закрытые — нет.
    let mut ours = bought("W1", &dx, Step::Stuck, ("20", "5"), Some("W1"));
    ours.at = dx.sell_on.clone();
    ours.dex_account = w("w1");
    let mut coming = bought("W5", &dx, Step::Transfer, ("20", "2"), Some("W5"));
    coming.dex_account = w("w1");
    arb::core::ledger::save(&core, &coming).await;
    assert!(apply_fact(&core, &mut ours, d("5")).await);
    assert_eq!(ours.qty, d("5"));
    coming.step = Step::Closed;
    arb::core::ledger::save(&core, &coming).await;
    let mut same = bought("W2", &dx, Step::Selling, ("20", "3"), Some("W1"));
    same.at = dx.sell_on.clone();
    same.dex_account = w("w1");
    let mut other = bought("W3", &dx, Step::Selling, ("20", "4"), Some("W3"));
    other.at = dx.sell_on.clone();
    other.dex_account = w("w2");
    for x in [&same, &other] {
        arb::core::ledger::save(&core, x).await;
    }
    let o = on_account(&core, &ours, &dx.sell_on);
    assert_eq!(o.deals, vec![("W2".to_string(), d("3"))]);
    assert!(o.unclear.is_empty());
    // Ревью 4, Н1: сделка в пути («неясная») не отключает вычет доказуемых. Кошелёк w7: Z продаёт 5000 (доказуемо на
    // нём), K в пути на него (100), Y застряла с учётом 1000 (фантом).
    let mut z = bought("Z", &dx, Step::Selling, ("20", "5000"), Some("Z"));
    z.at = dx.sell_on.clone();
    z.dex_account = w("w7");
    let mut k = bought("K", &dx, Step::Transfer, ("20", "100"), Some("K"));
    k.dex_account = w("w7");
    for x in [&z, &k] {
        arb::core::ledger::save(&core, x).await;
    }
    let mut y = bought("Y", &dx, Step::Stuck, ("20", "1000"), Some("Y"));
    y.at = dx.sell_on.clone();
    y.dex_account = w("w7");
    let o = on_account(&core, &y, &dx.sell_on);
    assert_eq!(o.deals, vec![("Z".to_string(), d("5000"))]);
    assert_eq!(o.unclear, vec!["K".to_string()]);
    // На кошельке 5600 — Y ведёт 600 (было бы 1000: 400 монет Z).
    assert!(apply_fact(&core, &mut y, d("5600")).await);
    assert_eq!(y.qty, d("600"));
    // На кошельке 5000 (всё — Z) — своих у Y нет: не продаёт и не закрывается, алерт человеку.
    assert!(!apply_fact(&core, &mut y, d("5000")).await);
    let rec = core.inner.deals.lock().unwrap().get("Y").cloned().unwrap();
    assert_eq!(rec.step, Step::WaitingHuman);
    assert_eq!(rec.qty, d("600"));
}

/// Ревью 4, Б2: площадка приняла вывод части «идёт» (ответ на запрос вывода — usdt_accepted.rs) — деньги списаны,
/// резерв части снимается сразу, без хеша; перегон другой сделки (другая монета, без докупки) уходит. Кусок закончился
/// (часть «идёт» стала остатком) — запись устарела; вывод отклонён после принятия — резерв снова целиком.
#[tokio::test]
async fn accepted_withdrawal_frees_reserve_without_hash() {
    let (core, _s, _j) = start("accepted").await;
    let (bingx, mexc) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let dx = dir_of("mexc", "bingx");
    let pending = |id: &str, amount: &str| {
        let mut x = bought(id, &dx, Step::Closed, ("40", "0"), None);
        x.usdt_return = Some(UsdtReturn::Pending {
            amount: d(amount),
            from: bingx.clone(),
            to: mexc.clone(),
        });
        x
    };
    arb::core::ledger::save(&core, &pending("X1", "500")).await;
    arb::core::ledger::save(&core, &pending("Y1", "300")).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("800"));
    // X1: BingX принял вывод 500 (хеша ещё нет): на BingX было 1000, стало 500.
    arb::core::usdt_accepted::mark(&core, "X1u", &bingx, d("500")).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("300"));
    let others = usdt_hold::held_except(&core, &bingx, None, Some("Y1"));
    assert_eq!(
        others.amount,
        Decimal::ZERO,
        "резерв X1 уже списан площадкой"
    );
    assert!(
        usdt_hold::spendable(d("500"), &others) >= d("300"),
        "перегон Y1 300 уходит"
    );
    // Кусок X1 закончился: часть «идёт» — остаток 200 (запись принятия устарела).
    arb::core::ledger::save(&core, &pending("X1", "200")).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("500"));
    // Новый кусок 200 принят, затем отклонён площадкой — деньги снова на площадке.
    arb::core::usdt_accepted::mark(&core, "X1up2", &bingx, d("200")).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("300"));
    arb::core::usdt_accepted::unmark(&core, "X1up2").await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("500"));
    // Ручной перевод USDT (не перегон сделки) — записи нет.
    arb::core::usdt_accepted::mark(&core, "tu20261118", &bingx, d("100")).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, d("500"));
}

/// Ревью 5, Н1: признак «вывод принят площадкой» — в учёте сделки (Deal.usdt_accepted): перезапуск («идёт» → «не
/// вернулись», usdt_return_state::on_restart) не возвращает в резерв уже списанные деньги; «не вернулись» без
/// принятого вывода — в резерве целиком (Тигран 18.11); часть закончена — записи сняты.
#[tokio::test]
async fn accepted_withdrawal_survives_restart() {
    let (core, _s, _j) = start("accepted-restart").await;
    let (bingx, mexc) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let dx = dir_of("mexc", "bingx");
    let mut x = bought("X1", &dx, Step::Closed, ("40", "0"), None);
    x.usdt_return = Some(UsdtReturn::Pending {
        amount: d("500"),
        from: bingx.clone(),
        to: mexc.clone(),
    });
    arb::core::ledger::save(&core, &x).await;
    arb::core::usdt_accepted::mark(&core, "X1u", &bingx, d("500")).await;
    let rec = core.inner.deals.lock().unwrap().get("X1").cloned().unwrap();
    assert_eq!(rec.usdt_accepted.len(), 1);
    // Запись учёта (база) — с признаком; разбор обратно — тот же.
    let back: Deal = serde_json::from_value(serde_json::to_value(&rec).unwrap()).unwrap();
    assert_eq!(back.usdt_accepted, rec.usdt_accepted);
    // Перезапуск: «идёт» → «не вернулись» на 500, признак остаётся — резерва нет (деньги ушли с BingX).
    let mut after = back;
    assert!(arb::core::usdt_return_state::on_restart(&core, &mut after).await);
    assert!(matches!(
        after.usdt_return,
        Some(UsdtReturn::NotReturned { .. })
    ));
    arb::core::ledger::save(&core, &after).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, Decimal::ZERO);
    // Перегон другой сделки Y1 (400) при свободных 500 уходит: резерв других — 0.
    let mut y = bought("Y1", &dx, Step::Closed, ("40", "0"), None);
    y.usdt_return = Some(UsdtReturn::Pending {
        amount: d("400"),
        from: bingx.clone(),
        to: mexc.clone(),
    });
    arb::core::ledger::save(&core, &y).await;
    let others = usdt_hold::held_except(&core, &bingx, None, Some("Y1"));
    assert!(usdt_hold::spendable(d("500"), &others) >= d("400"));
    // «Не вернулись» без принятого вывода — в резерве целиком.
    let mut n = bought("N1", &dx, Step::Closed, ("40", "0"), None);
    n.usdt_return = Some(UsdtReturn::NotReturned {
        amount: d("70"),
        at: bingx.clone(),
        why: "тест".into(),
    });
    arb::core::ledger::save(&core, &n).await;
    assert_eq!(
        usdt_hold::held_except(&core, &bingx, None, Some("Y1")).amount,
        d("70")
    );
    // «Перегнано вручную» по X1 — часть закончена, записи «вывод принят» сняты.
    arb::core::usdt_return_state::set(
        &core,
        "X1",
        UsdtReturn::Manual {
            amount: d("500"),
            at: bingx.clone(),
            to: mexc,
            by: "тест".into(),
            credited: None,
        },
        "",
    )
    .await;
    let rec = core.inner.deals.lock().unwrap().get("X1").cloned().unwrap();
    assert!(rec.usdt_accepted.is_empty());
}

/// Ревью 6, Н1 / М3: запись «вывод принят» части заменяется, а не копится; после перезапуска площадка отменила вывод
/// без живого цикла (unmark некому звать) — «Перегнать» (новая попытка части) снимает старую запись, резерв снова
/// целиком; старая запись учёта без поля usdt_accepted читается (пусто).
#[tokio::test]
async fn retry_clears_stale_accepted_and_old_record_reads() {
    let (core, _s, _j) = start("accepted-retry").await;
    let (bingx, mexc) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let dx = dir_of("mexc", "bingx");
    let mut x = bought("X1", &dx, Step::Closed, ("40", "0"), None);
    x.usdt_return = Some(UsdtReturn::Pending {
        amount: d("500"),
        from: bingx.clone(),
        to: mexc.clone(),
    });
    // Старая запись (без поля) читается: поля нет в JSON — пусто.
    let v = serde_json::to_value(&x).unwrap();
    assert!(v.get("usdt_accepted").is_none());
    let old: Deal = serde_json::from_value(v).unwrap();
    assert!(old.usdt_accepted.is_empty());
    arb::core::ledger::save(&core, &x).await;
    // Два ответа площадки по одной части — одна запись (последняя).
    arb::core::usdt_accepted::mark(&core, "X1u", &bingx, d("500")).await;
    arb::core::usdt_accepted::mark(&core, "X1u1", &bingx, d("500")).await;
    let rec = core.inner.deals.lock().unwrap().get("X1").cloned().unwrap();
    assert_eq!(rec.usdt_accepted.len(), 1);
    assert_eq!(rec.usdt_accepted[0].relay, "X1u1");
    // Перезапуск: «не вернулись» 500, резерв 0 (вывод был принят).
    let mut after = rec;
    assert!(arb::core::usdt_return_state::on_restart(&core, &mut after).await);
    arb::core::ledger::save(&core, &after).await;
    assert_eq!(usdt_hold::held(&core, &bingx, None).amount, Decimal::ZERO);
    // Площадка вывод отменила (живого цикла нет) — человек жмёт «Перегнать»: старая запись снята, резерв части — снова
    // её сумма (Перегнать — «идёт» 500).
    let r = core
        .command(arb::core::types::ManualCommand {
            action: arb::core::types::ManualAction::UsdtRetry {
                deal_id: "X1".into(),
            },
            source: arb::core::types::CommandSource::Ui,
            by: None,
            at: Utc::now(),
        })
        .await;
    assert!(r.is_ok(), "{r:?}");
    let rec = core.inner.deals.lock().unwrap().get("X1").cloned().unwrap();
    assert!(
        rec.usdt_accepted.is_empty(),
        "новая попытка — без старой записи"
    );
    assert!(usdt_hold::held(&core, &bingx, None).amount > Decimal::ZERO);
    let _ = mexc;
}

#[tokio::test]
async fn top_up_budget_excludes_first_invested() {
    let (core, s, _j) = start("budget").await;
    top_up(&s).await;
    set(&s, "money.max_per_trade_usd", json!("50")).await;
    set(&s, "money.max_per_coin_usd", json!("70")).await;
    set(&s, "money.max_in_transit_usd", json!("100")).await;
    let dx = dir_of("bingx", "bsc_dex");
    let before = limits_parts::parts(&core, &dx, &Mode::Live).unwrap();
    assert_eq!(before.cap(), d("50"));
    ride(
        &core,
        &bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1")),
    )
    .await;
    let p = limits_parts::parts(&core, &dx, &Mode::Live).unwrap();
    assert_eq!(p.coin_left, d("30"), "70 − вложено первой 40");
    assert_eq!(p.transit_left, d("60"), "100 − в пути 40");
    assert_eq!(p.cap(), d("30"));
    assert_eq!(p.binding(), "per_coin");
}

/// Стакан BingX CAKE-USDT из записи; bids без первых `off` монет (их продала первая сделка).
fn book(rec: &Rec, off: Decimal) -> OrderBook {
    let mut left = off;
    let mut bids = vec![];
    for l in &rec.bids {
        let take = left.min(l.qty);
        left -= take;
        if l.qty > take {
            bids.push(PriceLevel {
                price: l.price,
                qty: l.qty - take,
            });
        }
    }
    OrderBook {
        venue: VenueId("bingx".into()),
        asset: size_common::calc::direction().asset,
        bids,
        asks: rec.asks.clone(),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Котировка продажи по стакану (настоящая формула gateway::frozen), ставка — параметр проверки.
fn sell_quote(b: OrderBook, qty: Decimal) -> Quote {
    FrozenBook {
        venue: b.venue.clone(),
        asset: b.asset.clone(),
        side: Side::Sell,
        fee: BookFee::in_received(d("0.001"), Side::Sell, "CAKE", "USDT"),
        taken_at: Utc::now(),
        book: b,
    }
    .price(qty)
}

#[tokio::test]
async fn cex_sell_counts_book_after_first_deal_coins() {
    let rec = Rec::load("t179-cex").await;
    let depth: Decimal = rec.bids.iter().map(|l| l.qty).sum();
    // Округлены до 4 знаков: разность (off + qty) − off в Decimal — ровно qty.
    let (off, qty) = ((depth / d("3")).round_dp(4), (depth / d("4")).round_dp(4));
    let full = book(&rec, Decimal::ZERO);
    let m = ride_quote::marginal(
        &sell_quote(full.clone(), off + qty),
        &sell_quote(full.clone(), off),
        qty,
        off,
    );
    let seq = sell_quote(book(&rec, off), qty);
    assert_eq!(m.fillable_qty, seq.fillable_qty);
    let (mu, su) = (
        m.avg_price * m.fillable_qty,
        seq.avg_price * seq.fillable_qty,
    );
    assert!((mu - su).abs() <= su * d("0.000000000001"), "{mu} vs {su}");
    assert!(
        m.avg_price < sell_quote(full.clone(), qty).avg_price,
        "после первой — хуже, чем без неё"
    );
    let fee = |q: &Quote| {
        q.costs
            .iter()
            .find(|c| c.kind == CostKind::TradeFee)
            .unwrap()
            .amount
    };
    assert!((fee(&m) - fee(&seq)).abs() <= fee(&seq) * d("0.000000000001"));
    // Глубины не хватает на монеты первой и наши — исполнимо только остаток.
    let m2 = ride_quote::marginal(
        &sell_quote(full.clone(), depth + qty),
        &sell_quote(full, off),
        qty + depth,
        off,
    );
    assert_eq!(m2.fillable_qty, depth - off);
}

#[tokio::test]
async fn dex_sell_counts_pool_after_first_deal_coins() {
    let rec = Rec::load("t179-dex").await;
    let units = |x: Decimal| to_units(x, 18).unwrap();
    let out = |e: &arb::venues::bsc_dex::pools::registry::Entry, x: Decimal| {
        to_decimal(
            swap(&rec.ctx, e, CAKE, true, units(x)).unwrap().amount_out,
            18,
        )
        .unwrap()
    };
    let (off, qty) = (d("10"), d("5"));
    let q = |x: Decimal, usd: Decimal, gas: &str| Quote {
        venue: VenueId("bsc_dex".into()),
        asset: size_common::calc::direction().asset,
        side: Side::Sell,
        qty: x,
        fillable_qty: x,
        avg_price: usd / x,
        costs: vec![
            Cost::new(CostKind::Gas, d(gas), "BNB", false),
            Cost::new(CostKind::PoolFee, x * d("0.0025"), "CAKE", true),
        ],
        route: None,
        signal_route: None,
        pick: None,
        at: Utc::now(),
    };
    let (t, a) = (out(&rec.pool, off + qty), out(&rec.pool, off));
    let m = ride_quote::marginal(&q(off + qty, t, "0.0002"), &q(off, a, "0.0002"), qty, off);
    // Пул после продажи первой: резерв CAKE + off, резерв USDT − её выход.
    let mut after = rec.pool.clone();
    let PoolState::V2 { reserve0, reserve1 } = after.pool.state.clone() else {
        panic!("V2")
    };
    after.pool.state = PoolState::V2 {
        reserve0: reserve0 + units(off),
        reserve1: reserve1 - units(a),
    };
    let seq = out(&after, qty);
    let mine = m.avg_price * m.fillable_qty;
    assert_eq!(m.fillable_qty, qty);
    // Формула V2 с комиссией: разность и последовательная продажа совпадают до ~1e-7 доли (округление формулы).
    assert!((mine - seq).abs() <= seq * d("0.000001"), "{mine} vs {seq}");
    assert!(
        mine < out(&rec.pool, qty),
        "после первой — меньше, чем в нетронутый пул"
    );
    let gas = m
        .costs
        .iter()
        .find(|c| c.kind == CostKind::Gas)
        .unwrap()
        .amount;
    assert_eq!(gas, d("0.0002"), "газ — одного нашего обмена");
    let fee = m
        .costs
        .iter()
        .find(|c| c.kind == CostKind::PoolFee)
        .unwrap()
        .amount;
    assert_eq!(fee, qty * d("0.0025"), "комиссия пула — нашей части");
}

/// Маршрут DEX через пул по блоку.
fn route(pool: Address, block: u64) -> DexRoute {
    DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: "pancakeswap_v2".into(),
                kind: PoolKind::V2,
                pool: PoolRef::Contract(pool),
            }],
            expected_out: d("10"),
        }],
        block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    }
}

#[tokio::test]
async fn top_up_buy_needs_data_newer_than_group_buy() {
    let (core, s, _j) = start("fresh").await;
    top_up(&s).await;
    let dx = dir_of("bingx", "bsc_dex");
    let t = Utc::now();
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), Some("D1"));
    first.buy = Some(trade("D1", &dx, None, None, t));
    ride(&core, &first).await;
    let mut q = size_common::calc::calc(&Rec::load("t179-fresh").await, d("5")).buy_quote;
    q.at = t - Duration::milliseconds(1);
    assert!(
        ride_quote::buy_fresh(&core, &dx, &q)
            .unwrap_err()
            .contains("ждём кадр после неё")
    );
    q.at = t + Duration::milliseconds(1);
    assert!(ride_quote::buy_fresh(&core, &dx, &q).is_ok());
    // Покупка на DEX: сделка группы купила через пул PAIR в блоке 100.
    let dd = dir_of("bsc_dex", "mexc");
    let mut dex = bought("D2", &dd, Step::Transfer, ("20", "5"), Some("D2"));
    dex.buy = Some(trade("D2", &dd, Some(route(PAIR, 100)), Some(100), t));
    arb::core::ledger::save(&core, &dex).await;
    coin_ride::board(&core, &dd, "D2", Some("D2".into()));
    q.route = Some(route(PAIR, 99));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q)
            .unwrap_err()
            .contains("её покупка в блоке 100")
    );
    q.route = Some(route(PAIR, 100));
    assert!(ride_quote::buy_fresh(&core, &dd, &q).is_ok());
    q.route = Some(route(Address::repeat_byte(7), 1));
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q).is_ok(),
        "другой пул — проверки нет"
    );
    q.route = None;
    assert!(
        ride_quote::buy_fresh(&core, &dd, &q)
            .unwrap_err()
            .contains("агрегатор")
    );
    // Монеты идущих сделок группы, которые будут проданы на bsc_dex: только D1 (D2 продаёт на mexc).
    assert_eq!(
        ride_quote::sell_ahead(&core, &dir_of("mexc", "bsc_dex")),
        d("10")
    );
}

#[tokio::test]
async fn off_no_group_rides_change_nothing() {
    let (core, _s, _j) = start("off-quotes").await;
    let dx = dir_of("bingx", "bsc_dex");
    let mut first = bought("D1", &dx, Step::Transfer, ("40", "10"), None);
    first.buy = Some(trade("D1", &dx, None, None, Utc::now()));
    ride(&core, &first).await;
    let mut q = size_common::calc::calc(&Rec::load("t179-off").await, d("5")).buy_quote;
    q.at = Utc::now() - Duration::hours(1);
    // Идущая сделка без группы (теневой замер 267 и т. п.) — котировки как раньше.
    assert!(ride_quote::buy_fresh(&core, &dx, &q).is_ok());
    assert_eq!(ride_quote::sell_ahead(&core, &dx), Decimal::ZERO);
}

#[tokio::test]
async fn trail_waits_for_book_after_group_sale() {
    let (core, s, _j) = start("trail").await;
    top_up(&s).await;
    let dx = dir_of("bsc_dex", "bingx");
    let t = Utc::now();
    let mut x = bought("D1", &dx, Step::Closed, ("40", "0"), Some("D1"));
    let mut sale = trade("D1", &dx, None, None, t);
    sale.request.venue = dx.sell_on.clone();
    sale.request.side = Side::Sell;
    x.sells.push(sale);
    arb::core::ledger::save(&core, &x).await;
    let key = CoinKey::of(&dx.asset);
    let mut q = size_common::calc::calc(&Rec::load("t179-trail").await, d("5")).sell_quote;
    q.at = t;
    assert!(
        ride_trail::check(&core, &key, &dx.sell_on, &q)
            .unwrap_err()
            .contains("продажи сделки D1")
    );
    q.at = t + Duration::milliseconds(1);
    assert!(ride_trail::check(&core, &key, &dx.sell_on, &q).is_ok());
    q.at = t - Duration::seconds(1);
    assert!(
        ride_trail::check(&core, &key, &dx.sell_on, &q).is_ok(),
        "след снят"
    );
}

/// USDT кошелька bsc_dex (адрес — записи tests/size_common).
fn usdt() -> Asset {
    Asset {
        chain: None,
        ticker: "USDT".into(),
        contract: Some(size_common::USDT),
    }
}

/// Поступление без хеша (как поток BingX): сумма.
fn no_hash(amount: &str) -> BalanceChange {
    BalanceChange {
        venue: VenueId("bsc_dex".into()),
        asset: usdt(),
        delta: d(amount),
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: None,
        at: Utc::now(),
        source: "проверка t179".into(),
        account: None,
    }
}

/// Сценарий ревью Б2: сделка 1 ждёт 100, сделка 2 — 95, у первой дошло 95 (без хеша). Возвращает: закрыто ли у
/// сделки 2 и было ли событие у какой-либо.
async fn two_expectations(on: bool) -> (bool, bool) {
    let (core, s, _j) = start(if on { "credit-on" } else { "credit-off" }).await;
    if on {
        top_up(&s).await;
    }
    let v = Scope::Venue("bsc_dex".into());
    s.set(
        "transfer.credit_amount_tolerance_pct",
        v.clone(),
        Some(json!("0")),
        "тест",
    )
    .await
    .unwrap();
    s.set("transfer.credit_by_event", v, Some(json!(true)), "тест")
        .await
        .unwrap();
    let from = VenueId("bingx".into());
    let to = VenueId("bsc_dex".into());
    let t1 = credit_watch::register(&core, "D1", &from, &to, &usdt(), d("100"));
    let t2 = credit_watch::register(&core, "D2", &from, &to, &usdt(), d("95"));
    credit_event::on_change(&core, no_hash("95")).await;
    if on {
        tokio::time::timeout(StdDuration::from_millis(10), t1.wake.notified())
            .await
            .expect("опрос сделки 1 разбужен");
    }
    (
        transfer_credit::by_event(Some(&t2)).is_some(),
        t1.has_event() || t2.has_event(),
    )
}

#[tokio::test]
async fn credit_without_hash_two_expectations() {
    // Выключено — как раньше: подходящее по сумме ожидание (сделка 2) закрывается событием.
    assert_eq!(two_expectations(false).await, (true, true));
    // Включено — событие без хеша ни к одному не прикреплено, решает опрос по хешу.
    assert_eq!(two_expectations(true).await, (false, false));
}

/// Закрытая обычной продажей на площадке плана сделка группы `g` (выручка на sell_on). Ошибок нет.
fn sold(id: &str, dir: &arb::core::types::Direction, g: &str) -> Deal {
    let mut x = bought(id, dir, Step::Closed, ("40", "0"), Some(g));
    x.at = dir.sell_on.clone();
    x.result_usd = Some(d("1"));
    x.sells.push(trade(id, dir, None, None, Utc::now()));
    x
}

/// Сделки группы `g` на шаге продажи, учтённые в идущих (как после board). Ошибок нет.
async fn riding(core: &Core, dx: &arb::core::types::Direction, g: &str, ids: &[&str]) {
    for id in ids {
        let x = bought(id, dx, Step::Selling, ("40", "10"), Some(g));
        arb::core::ledger::save(core, &x).await;
        coin_ride::board(core, dx, id, Some(g.into()));
    }
}

/// Ждёт строку в хронологии сделки (общий возврат — отдельной задачей). Возвращает: сделку. Тест падает по сроку.
async fn wait_note(core: &Core, id: &str, part: &str) -> Deal {
    for _ in 0..200 {
        let d = core.inner.deals.lock().unwrap().get(id).cloned().unwrap();
        if d.timeline.iter().any(|(_, t)| t.contains(part)) {
            return d;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("нет строки «{part}» у {id}");
}

/// Закрыть сделку группы так, как это делает ledger::finish: части возврата (hold_parts), сохранение, после
/// сохранения — общий возврат группы (after_close). Возвращает: части, перегоняемые этой сделкой сразу (своё).
async fn close(core: &Core, x: &mut Deal, amount: &str) -> Vec<(VenueId, Decimal)> {
    let own = coin_group::hold_parts(x, vec![(x.direction.sell_on.clone(), d(amount))]);
    arb::core::ledger::save(core, x).await;
    coin_group::after_close(core, x).await;
    own
}

/// Есть ли строка в хронологии сделки из учёта. Ошибок нет.
fn has_note(core: &Core, id: &str, part: &str) -> bool {
    core.inner
        .deals
        .lock()
        .unwrap()
        .get(id)
        .is_some_and(|d| d.timeline.iter().any(|(_, t)| t.contains(part)))
}

#[tokio::test]
async fn group_return_once_lead_last() {
    let (core, s, _j) = start("group-last").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    // Вложенная закрылась первой — её возврат ждёт (0 сейчас), общего возврата ещё нет.
    let mut x = sold("D2", &dx, "G");
    assert!(close(&core, &mut x, "95").await.is_empty());
    assert!(x.group.as_ref().unwrap().wait.is_some());
    // Первая — последняя: один общий возврат 195, носитель — первая, у вложенной — «в сделке G», wait снят.
    let mut g = sold("G", &dx, "G");
    assert!(close(&core, &mut g, "100").await.is_empty());
    assert!(has_note(&core, "G", "общий возврат USDT группы G: 195"));
    let sum = g.group.as_ref().unwrap().sum.clone().unwrap();
    assert_eq!(
        (sum.deals, sum.others, sum.usdt),
        (vec!["D2".to_string()], d("1"), d("95"))
    );
    let x = core.inner.deals.lock().unwrap().get("D2").cloned().unwrap();
    assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("G"));
    assert!(x.group.as_ref().unwrap().wait.is_none());
    // Н1: «Перегнать» по сделке, чьи USDT ушли общим возвратом, — отказ (второго вывода нет).
    let why = manual_usdt::retry_block(&x, &dx.sell_on, &dx.buy_on).unwrap();
    assert!(
        why.contains("общего возврата группы докупки в сделке G"),
        "{why}"
    );
}

#[tokio::test]
async fn last_deal_leaving_group_still_flushes_parts() {
    let (core, s, _j) = start("group-left").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    let mut g = sold("G", &dx, "G");
    assert!(close(&core, &mut g, "100").await.is_empty());
    assert!(!has_note(&core, "G", "общий возврат USDT группы"));
    // Ревью 2, Б1: последняя закрылась перегоном — свой возврат, а отложенные 100 USDT первой уходят сразу.
    let mut x = sold("D2", &dx, "G");
    x.relays = 1;
    assert_eq!(close(&core, &mut x, "95").await.len(), 1);
    assert!(x.group.as_ref().unwrap().left);
    assert!(has_note(&core, "G", "общий возврат USDT группы G: 100"));
}

#[tokio::test]
async fn top_up_before_buy_holds_group_return() {
    let (core, s, _j) = start("group-pending").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let key = CoinKey::of(&dx.asset);
    riding(&core, &dx, "G", &["G"]).await;
    // Ревью 2, Б2: вложенная вошла (admit), ещё не купила — общий возврат ждёт её.
    coin_group::join(&core, "G");
    let mut g = sold("G", &dx, "G");
    close(&core, &mut g, "100").await;
    assert!(!has_note(&core, "G", "общий возврат USDT группы"));
    // Не состоялась (отказ предпроверки) — выходит, общий возврат уходит без неё.
    coin_group::dropped(&core, &key, "G");
    wait_note(&core, "G", "общий возврат USDT группы G: 100").await;
}

#[tokio::test]
async fn top_up_after_buy_joins_group_return() {
    let (core, s, _j) = start("group-joined").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G"]).await;
    coin_group::join(&core, "G");
    let mut g = sold("G", &dx, "G");
    close(&core, &mut g, "100").await;
    // Купила и в идущих — общий возврат ждёт её продажи.
    riding(&core, &dx, "G", &["D2"]).await;
    coin_group::joined(&core, "G");
    assert!(!has_note(&core, "G", "общий возврат USDT группы"));
    let mut x = sold("D2", &dx, "G");
    close(&core, &mut x, "95").await;
    assert!(has_note(&core, "G", "общий возврат USDT группы G: 195"));
}

#[tokio::test]
async fn two_group_deals_closing_at_once_return_once() {
    let (core, s, _j) = start("group-race").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    let (mut g, mut x) = (sold("G", &dx, "G"), sold("D2", &dx, "G"));
    let (b, _) = tokio::join!(close(&core, &mut g, "100"), close(&core, &mut x, "95"));
    assert!(b.is_empty());
    let n = ["G", "D2"]
        .iter()
        .filter(|id| has_note(&core, id, "общий возврат USDT группы G: 195"))
        .count();
    assert_eq!(n, 1, "ровно один общий возврат");
}

#[tokio::test]
async fn three_group_deals_closing_at_once_return_once() {
    let (core, s, _j) = start("group-race3").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2", "D3"]).await;
    let (mut g, mut x, mut y) = (
        sold("G", &dx, "G"),
        sold("D2", &dx, "G"),
        sold("D3", &dx, "G"),
    );
    tokio::join!(
        close(&core, &mut g, "100"),
        close(&core, &mut x, "95"),
        close(&core, &mut y, "90")
    );
    let n = ["G", "D2", "D3"]
        .iter()
        .filter(|id| has_note(&core, id, "общий возврат USDT группы G: 285"))
        .count();
    assert_eq!(n, 1, "ровно один общий возврат на полную сумму");
    for id in ["D2", "D3"] {
        let x = core.inner.deals.lock().unwrap().get(id).cloned().unwrap();
        assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("G"));
    }
}

#[test]
fn group_return_split_by_withdraw_max() {
    let v = VenueId("bingx".into());
    let p = coin_group::pieces(&v, d("250"), Some(d("100")));
    assert_eq!(p.len(), 3);
    assert!(p.iter().all(|(_, a)| *a <= d("100")));
    assert_eq!(p.iter().map(|(_, a)| *a).sum::<Decimal>(), d("250"));
    assert_eq!(
        coin_group::pieces(&v, d("80"), Some(d("100"))),
        vec![(v.clone(), d("80"))]
    );
    assert_eq!(coin_group::pieces(&v, d("80"), None), vec![(v, d("80"))]);
}

#[tokio::test]
async fn stuck_last_deal_does_not_hold_group_return() {
    let (core, s, _j) = start("group-stuck").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    let mut g = sold("G", &dx, "G");
    close(&core, &mut g, "100").await;
    // Последняя идущая застряла — её не ждём: отложенная выручка уходит общим возвратом сразу (носитель — G).
    let stuck = bought("D2", &dx, Step::Stuck, ("40", "10"), Some("G"));
    arb::core::ledger::save(&core, &stuck).await;
    wait_note(&core, "G", "общий возврат USDT группы G: 100").await;
}

#[test]
fn won_deal_skips_direction_pause() {
    let dx = dir_of("bingx", "bsc_dex");
    let mut x = bought("D1", &dx, Step::Closed, ("40", "0"), Some("D1"));
    x.at = dx.sell_on.clone();
    x.sells.push(trade("D1", &dx, None, None, Utc::now()));
    x.result_usd = Some(d("0.5"));
    assert!(route_pause::won(&x));
    x.result_usd = Some(d("-0.01"));
    assert!(!route_pause::won(&x), "убыток — пауза");
    x.result_usd = Some(d("0.5"));
    x.relays = 1;
    assert!(!route_pause::won(&x), "перегон — пауза");
    x.relays = 0;
    x.sells.clear();
    assert!(!route_pause::won(&x), "без продажи бота — пауза");
}

/// USDT BEP20 BingX из записи config/getall 24.10 (разбор адаптера): (комиссия вывода, максимум вывода).
fn bingx_usdt_bep20() -> (Decimal, Decimal) {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/config_getall.json").unwrap();
    let b: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let list =
        arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &VenueId("bingx".into()))
            .unwrap();
    let i = list.iter().find(|x| x.asset.ticker == "USDT").unwrap();
    (i.withdraw_fee.unwrap(), i.withdraw_max.unwrap())
}

/// Итог перегона куска по записи: ушло — кусок, зачислено — кусок минус комиссия вывода площадки.
fn piece_relayed(piece: Decimal, fee: Decimal) -> Relayed {
    Relayed {
        sent: piece,
        credited: piece - fee,
        fee_usd: None,
        fee_note: None,
    }
}

/// Носитель общего возврата `total` с биржи B (bingx) на A: закрыт, итог 1, часть учёта «идёт» на всю сумму.
fn bearer(total: Decimal) -> Deal {
    let dx = dir_of("mexc", "bingx");
    let mut x = sold("G", &dx, "G");
    usdt_return_parts::put(
        &mut x,
        UsdtReturn::Pending {
            amount: total,
            from: dx.sell_on.clone(),
            to: dx.buy_on.clone(),
        },
    );
    x
}

#[test]
fn group_return_pieces_all_arrive_one_part() {
    let (fee, max) = bingx_usdt_bep20();
    let (from, to) = (VenueId("bingx".into()), VenueId("mexc".into()));
    for k in [2usize, 3] {
        // Сумма выше максимума вывода: k кусков, каждый ≤ максимума (записанный максимум BingX).
        let total = max * Decimal::from(k - 1) + max / d("2");
        let split = coin_group::pieces(&from, total, Some(max));
        assert_eq!(split.len(), k);
        let mut x = bearer(total);
        let mut ids = std::collections::HashSet::new();
        let mut done = Decimal::ZERO;
        for (i, (_, piece)) in split.iter().enumerate() {
            assert!(
                ids.insert(usdt_return_pieces::piece_id(&x.id, "", i + 1)),
                "ID кусков разные"
            );
            let rest = usdt_return_pieces::ok(
                &mut x,
                (&from, &to),
                total,
                *piece,
                &piece_relayed(*piece, fee),
            );
            done += *piece;
            assert_eq!(rest, total - done);
            // Одна часть учёта на площадку: куски не затирают друг друга.
            assert!(x.usdt_return_parts.is_empty());
            if i + 1 < k {
                assert_eq!(
                    x.usdt_return,
                    Some(UsdtReturn::Pending {
                        amount: rest,
                        from: from.clone(),
                        to: to.clone()
                    })
                );
                assert_eq!(
                    x.group.as_ref().unwrap().pieces.as_ref().unwrap().n as usize,
                    i + 1
                );
                // Затраты дошедших — уже в известных.
                assert_eq!(deal_return_cost::known(&x), fee * Decimal::from(i + 1));
            }
        }
        // Все дошли: «вернулись» на всю сумму, затраты — комиссия каждого куска (п. 14).
        let Some(UsdtReturn::Done {
            amount,
            credited,
            sent,
            ..
        }) = x.usdt_return.clone()
        else {
            panic!("вернулись")
        };
        assert_eq!((amount, sent), (total, Some(total)));
        assert_eq!(credited, total - fee * Decimal::from(k));
        assert!(x.group.as_ref().unwrap().pieces.is_none());
        assert!(usdt_return_parts::all_done(&x));
        deal_return_cost::apply(&mut x);
        assert_eq!(x.result_usd, Some(d("1") - fee * Decimal::from(k)));
        assert_eq!(deal_return_cost::known(&x), fee * Decimal::from(k));
    }
}

#[test]
fn group_return_middle_piece_fails_exact_rest() {
    let (fee, max) = bingx_usdt_bep20();
    let (from, to) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let total = max * d("2") + max / d("2");
    let split = coin_group::pieces(&from, total, Some(max));
    assert_eq!(split.len(), 3);
    let mut x = bearer(total);
    let p1 = split[0].1;
    usdt_return_pieces::ok(&mut x, (&from, &to), total, p1, &piece_relayed(p1, fee));
    // Второй кусок отказан — «не вернулись» на точный остаток (он и третий), первый учтён.
    let rest = usdt_return_pieces::failed(&mut x, &from, total, "кусок 2 из 3 не дошёл");
    assert_eq!(rest, total - p1);
    assert_eq!(
        x.usdt_return,
        Some(UsdtReturn::NotReturned {
            amount: total - p1,
            at: from.clone(),
            why: "кусок 2 из 3 не дошёл".into()
        })
    );
    assert!(!usdt_return_parts::all_done(&x));
    assert!(
        !deal_return_cost::in_progress(&x),
        "возврат не «идёт» — уведомления уходят"
    );
    assert_eq!(deal_return_cost::known(&x), fee);
    deal_return_cost::apply(&mut x);
    assert_eq!(x.result_usd, Some(d("1") - fee));
    assert!(
        x.timeline
            .iter()
            .any(|(_, t)| t.contains("USDT не вернулись"))
    );
}

#[tokio::test]
async fn group_return_restart_mid_pieces_counts_rest() {
    let (core, s, _j) = start("pieces-restart").await;
    top_up(&s).await;
    let (fee, max) = bingx_usdt_bep20();
    let (from, to) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let total = max + max / d("2");
    let split = coin_group::pieces(&from, total, Some(max));
    let mut x = bearer(total);
    let p1 = split[0].1;
    usdt_return_pieces::ok(&mut x, (&from, &to), total, p1, &piece_relayed(p1, fee));
    deal_return_cost::apply(&mut x);
    arb::core::ledger::save(&core, &x).await;
    // Перезапуск посреди кусков: остаток — «не вернулись» целиком, дошедший кусок учтён (ничего «нигде не числится»).
    arb::core::ledger::load(&core).await;
    let y = core.inner.deals.lock().unwrap().get("G").cloned().unwrap();
    let Some(UsdtReturn::NotReturned { amount, at, .. }) = y.usdt_return.clone() else {
        panic!("не вернулись")
    };
    assert_eq!((amount, at), (total - p1, from));
    assert_eq!(
        y.group.as_ref().unwrap().pieces.as_ref().unwrap().amount,
        p1
    );
    assert_eq!(y.result_usd, Some(d("1") - fee));
}

#[tokio::test]
async fn restart_after_bearer_saved_keeps_copy_in_shared_return() {
    let (core, s, _j) = start("flush-order").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    // Ревью 3, Н2: носитель сохранён (сумма группы, «идёт» на 195), вложенная D2 — ещё нет (wait на 95).
    let mut g = bearer(d("195"));
    g.group.as_mut().unwrap().sum = Some(arb::core::deal_group::GroupSum {
        deals: vec!["D2".into()],
        others: d("1"),
        usdt: d("95"),
    });
    let mut x = sold("D2", &dx, "G");
    x.group.as_mut().unwrap().wait = Some((dx.sell_on.clone(), d("95")));
    arb::core::ledger::save(&core, &x).await;
    arb::core::ledger::save(&core, &g).await;
    arb::core::ledger::load(&core).await;
    let deals = core.inner.deals.lock().unwrap().clone();
    let x = deals.get("D2").unwrap();
    assert_eq!(x.group.as_ref().unwrap().shared_in.as_deref(), Some("G"));
    assert!(x.group.as_ref().unwrap().wait.is_none());
    assert!(
        x.usdt_return.is_none(),
        "выручка D2 — в возврате носителя, не «не вернулись»"
    );
    let Some(UsdtReturn::NotReturned { amount, .. }) = deals.get("G").unwrap().usdt_return.clone()
    else {
        panic!("носитель — «не вернулись»")
    };
    assert_eq!(amount, d("195"));
}

#[tokio::test]
async fn admit_join_holds_group_return() {
    let (core, s, _j) = start("admit-join").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let key = CoinKey::of(&dx.asset);
    riding(&core, &dx, "G", &["G"]).await;
    // Ревью 3, Н1: вложенная учтена в группе в самом admit — закрытие последней идущей её ждёт.
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Join("G".into())));
    let mut g = sold("G", &dx, "G");
    close(&core, &mut g, "100").await;
    assert!(!has_note(&core, "G", "общий возврат USDT группы"));
    coin_ride::release(&core, &key, None);
    coin_group::dropped(&core, &key, "G");
    wait_note(&core, "G", "общий возврат USDT группы G: 100").await;
}

#[tokio::test]
async fn flushed_group_takes_no_new_top_ups() {
    let (core, s, _j) = start("flushed").await;
    top_up(&s).await;
    let dx = dir_of("mexc", "bingx");
    let key = CoinKey::of(&dx.asset);
    riding(&core, &dx, "G", &["G", "D2"]).await;
    assert_eq!(coin_ride::group_running(&core, &dx).as_deref(), Some("G"));
    let mut g = sold("G", &dx, "G");
    close(&core, &mut g, "100").await;
    // D2 застряла — общий возврат группы ушёл; она «была частью группы» сразу (п. 13).
    let stuck = bought("D2", &dx, Step::Stuck, ("40", "10"), Some("G"));
    arb::core::ledger::save(&core, &stuck).await;
    wait_note(&core, "G", "общий возврат USDT группы G: 100").await;
    assert!(coin_group::flushed(&core, "G"));
    assert!(has_note(&core, "D2", "Была частью группы докупки G"));
    // Задача G кончилась; D2 вернулась на путь (копия исполнителя без пометки — пометка остаётся).
    coin_ride::release(&core, &key, Some("G"));
    coin_group::released(&core, &key, "G");
    let back = bought("D2", &dx, Step::Selling, ("40", "10"), Some("G"));
    arb::core::ledger::save(&core, &back).await;
    let d2 = core.inner.deals.lock().unwrap().get("D2").cloned().unwrap();
    assert!(d2.group.as_ref().unwrap().left);
    // Ревью 3, Н3: новая сделка того же направления — первая новой группы (фиксированная часть — в её плане).
    assert_eq!(coin_ride::group_running(&core, &dx), None);
    assert_eq!(coin_ride::admit(&core, &dx), Ok(Seat::Lead));
}

#[test]
fn group_total_once_for_notified_bearer() {
    // Ревью 3, Н4: носитель — вложенная, уже известившая (отложенного deal.closed нет).
    let mut x = bearer(d("195"));
    x.group.as_mut().unwrap().sum = Some(arb::core::deal_group::GroupSum {
        deals: vec!["D2".into()],
        others: d("1"),
        usdt: d("95"),
    });
    group_notice::mark(&mut x);
    assert!(x.group.as_ref().unwrap().total_due);
    assert!(
        group_notice::take(&mut x).is_none(),
        "возврат идёт — ещё рано"
    );
    usdt_return_parts::put(
        &mut x,
        UsdtReturn::Done {
            amount: d("195"),
            credited: d("194"),
            from: VenueId("bingx".into()),
            to: VenueId("mexc".into()),
            sent: Some(d("195")),
            fee_usd: None,
        },
    );
    let t = group_notice::take(&mut x).unwrap();
    assert!(t.contains("Итог группы"), "{t}");
    assert!(group_notice::take(&mut x).is_none(), "ровно один раз");
    // Носитель с отложенным deal.closed (сделка 1) — итог в его deal.closed, отдельного нет.
    let mut y = bearer(d("195"));
    y.group.as_mut().unwrap().sum = x.group.as_ref().unwrap().sum.clone();
    y.closed_notice = Some("продано".into());
    group_notice::mark(&mut y);
    assert!(!y.group.as_ref().unwrap().total_due);
    // Вышедшая с чужой выручкой — текст говорит, что вёз и выручку группы.
    let mut z = x.group.as_deref().unwrap().clone();
    z.left = true;
    let lines = z.lines("G", Some(d("1")));
    assert!(
        lines
            .iter()
            .any(|(_, v)| v.contains("в нём и выручка сделок D2")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn stuck_then_sold_on_plan_venue_returns_own() {
    let (core, s, _j) = start("left-own").await;
    top_up(&s).await;
    set(&s, "usdt_return.mode", json!("per_trade")).await;
    let dx = dir_of("mexc", "bingx");
    riding(&core, &dx, "G", &["G", "D2"]).await;
    // D2 застряла — в учёте «была частью группы».
    let stuck = bought("D2", &dx, Step::Stuck, ("40", "10"), Some("G"));
    arb::core::ledger::save(&core, &stuck).await;
    // Ревью 4, Н7: режим застрявшей монеты продал на B без перегона — закрывает копия задачи без пометки.
    let mut x = sold("D2", &dx, "G");
    x.proceeds = d("95");
    x.sells[0].request.venue = dx.sell_on.clone();
    x.sells[0].request.side = Side::Sell;
    assert!(!x.group.as_ref().unwrap().left);
    usdt_return_deal::prepare(&core, &mut x).await;
    let g = x.group.as_ref().unwrap();
    assert!(g.left, "пометка из учёта");
    assert!(
        g.wait.is_none(),
        "в общий возврат группы не ушла — свой возврат"
    );
    assert!(x.usdt_return.is_some(), "своя часть возврата");
    let lines = g.lines("D2", x.result_usd);
    assert!(
        lines.iter().any(|(_, v)| v.contains("свой возврат USDT")),
        "{lines:?}"
    );
}

#[test]
fn manual_retry_pieces_continue_from_done() {
    // Ревью 4, Н8: кусок 1 из 3 дошёл, 2 отказал; «Перегнать» остатка — кусками, сумма общего возврата = дошедшее +
    // остаток (usdt_return_pieces::run), учёт продолжается.
    let (fee, max) = bingx_usdt_bep20();
    let (from, to) = (VenueId("bingx".into()), VenueId("mexc".into()));
    let total = max * d("2") + max / d("2");
    let split = coin_group::pieces(&from, total, Some(max));
    let mut x = bearer(total);
    let p1 = split[0].1;
    usdt_return_pieces::ok(&mut x, (&from, &to), total, p1, &piece_relayed(p1, fee));
    let rest = usdt_return_pieces::failed(&mut x, &from, total, "кусок 2 из 3 не дошёл");
    let again = coin_group::pieces(&from, rest, Some(max));
    assert_eq!(again.len(), 2, "остаток выше максимума — кусками");
    let base = x.group.as_ref().unwrap().pieces.as_ref().unwrap().amount;
    for (_, p) in &again {
        usdt_return_pieces::ok(
            &mut x,
            (&from, &to),
            base + rest,
            *p,
            &piece_relayed(*p, fee),
        );
    }
    let Some(UsdtReturn::Done {
        amount, credited, ..
    }) = x.usdt_return.clone()
    else {
        panic!("вернулись")
    };
    assert_eq!(amount, total);
    assert_eq!(credited, total - fee * d("3"));
    assert_eq!(deal_return_cost::known(&x), fee * d("3"));
    assert!(x.group.as_ref().unwrap().pieces.is_none());
    // ID кусков «Перегнать» — со своей меткой попытки.
    assert_ne!(
        usdt_return_pieces::piece_id("G", "m123", 2),
        usdt_return_pieces::piece_id("G", "", 2)
    );
}

#[test]
fn pieces_never_exceed_max() {
    // Ревью 4, Н10: максимум из записи BingX, максимум с 9 знаками (точнее округления куска) и круглый — ни один кусок
    // не больше максимума, сумма — ровно total, каждый > 0.
    let (_, rec) = bingx_usdt_bep20();
    let v = VenueId("bingx".into());
    for max in [rec, d("999.999999999"), d("100")] {
        let eps = d("0.00000001");
        for total in [
            max + eps,
            max * d("2.5"),
            max * d("3"),
            max * d("3") - eps,
            max * d("3") + eps,
            max * d("7") + d("0.123456789"),
        ] {
            let p = coin_group::pieces(&v, total, Some(max));
            assert!(p.len() >= 2, "{total} / {max}");
            assert!(
                p.iter().all(|(_, a)| *a <= max && *a > Decimal::ZERO),
                "{p:?} max {max}"
            );
            assert_eq!(p.iter().map(|(_, a)| *a).sum::<Decimal>(), total);
        }
    }
}

#[test]
fn relay_id_owner_finds_deal() {
    let id = "20261117120000000CAKE".to_string();
    let next = "20261117120000000CAKE1".to_string();
    let deals = [id.clone(), next.clone()];
    for (relay, want) in [
        (format!("{id}u"), Some(&id)),
        (format!("{id}u3"), Some(&id)),
        (format!("{id}m123456"), Some(&id)),
        (format!("{id}up2"), Some(&id)),
        (format!("{id}m123456up2"), Some(&id)),
        (format!("{next}u"), Some(&next)),
        (format!("{next}up1"), Some(&next)),
        (id.clone(), Some(&id)),
        ("tu20261117120000000".to_string(), None),
        (format!("{id}x"), None),
        (format!("{id}m"), None),
        (format!("{id}up"), None),
    ] {
        assert_eq!(
            relay_owner::owner(deals.iter(), &relay).as_ref(),
            want,
            "{relay}"
        );
    }
}

#[tokio::test]
async fn failed_relay_gas_goes_to_deal() {
    let (core, _s, _j) = start("relay-gas").await;
    let x = bearer(d("195"));
    arb::core::ledger::save(&core, &x).await;
    // Газ неудачного перегона USDT куска общего возврата: сделка находится по ID перегона (п. 14).
    let mut gas = vec![arb::core::deal_gas::Wasted {
        what: "откат перевода".into(),
        kind: CostKind::TransferGas,
        bnb: d("0.0001"),
    }];
    arb::core::deal_gas::charge_closed(
        &core,
        &format!("{}up2", x.id),
        &x.direction.sell_on,
        &mut gas,
    )
    .await;
    assert!(has_note(&core, "G", "откат перевода"));
}
