//! Файл: tests/bsc_dex_signal_epoch_352.rs
//! Что делает: решение 352 — эпоха котировки DEX (venues/bsc_dex/signal_epoch.rs) и её основа на записанных пулах CAKE
//! (tests/data/routes_2026-10-29 и routes_2026-10-30: шесть пулов V2/V3 — CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB,
//! BTCB-WBNB; route_common). Проверяет:
//! (1) выбор маршрута (quote/routing.rs run — то, что котировка DEX считает после газа и decimals) детерминирован: на
//!     том же состоянии повторная котировка равна прежней побитово (JSON без времени котировки at) — классика и граф,
//!     покупка и продажа, несколько объёмов; повторная котировка не меняет часть эпохи state (граф и глубина уже
//!     посчитаны) — значит память с этой эпохой отдаёт ровно то, что дала бы котировка заново;
//! (2) state меняется при изменении каждого входа: состояние пула монеты (CAKE-WBNB из записи другого блока), пула
//!     промежуточной пары (WBNB-USDT из записи другого блока), фактов проверок пула, настроек, списка промежуточных,
//!     набора путей монеты в кэше (dex.route_paths_ttl_ms);
//! (3) без свежей головы и цены газа этой головы эпохи нет (gate None) — котировка всегда заново;
//! (4) счётчики поколений растут на каждое изменение: check/state.rs (set_coin, update_pool), settings/store.rs (set);
//!     цена газа в памяти (quote/gas.rs known) до первого запроса к узлу — нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{signal_epoch, quote::routing, quote::route_cache}, tests/route_common.
//! Моков нет: состояния пулов — записи узла; узел не вызывается.

mod route_common;

use std::collections::HashSet;

use alloy_primitives::U256;
use arb::core::types::{Asset, CheckState, CoinCheck, Side, TokenTaxes, VenueId};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::quote::route_cache::{self, Search};
use arb::venues::bsc_dex::quote::routing::{self, Input};
use arb::venues::bsc_dex::signal_epoch;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use route_common::{BTCB, CAKE, USDT, WBNB};

/// Цена газа, wei (как в tests/bsc_dex_split.rs).
const GAS: u64 = 1_000_000_000;

fn cake() -> Asset {
    Asset {
        chain: None,
        ticker: "CAKE".into(),
        contract: Some(CAKE),
    }
}

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Входные данные выбора маршрута, как их собирает quote/mod.rs priced_at.
fn input(p: &Parts, side: Side, qty: Decimal) -> Input {
    Input {
        asset: cake(),
        side,
        qty,
        coin: CAKE,
        usdt: USDT,
        taxes: zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: p.registry.by_pair(CAKE, USDT),
        mids: p
            .mids
            .active(&p.ctx)
            .into_iter()
            .map(|m| (m, 18u8))
            .collect(),
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart: false,
    }
}

/// Котировка выбора маршрута JSON без времени (at); ошибка — её текст.
async fn route(p: &Parts, side: Side, qty: Decimal) -> Value {
    match routing::run(p.quoting(), input(p, side, qty)).await {
        Ok(q) => {
            let mut v = serde_json::to_value(q).unwrap();
            v.as_object_mut().unwrap().remove("at");
            v
        }
        Err(e) => json!({ "error": e.message }),
    }
}

async fn set(p: &Parts, name: &str, v: Value) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(name, s, Some(v), "тест").await.unwrap();
}

/// Часть эпохи без узла и времени.
fn state(p: &Parts) -> String {
    signal_epoch::state(p, CAKE).expect("state собирается: канал отправки задан")
}

#[tokio::test]
async fn same_state_same_quote_classic_and_graph() {
    let p = route_common::parts().await;
    route_common::channel(&p, "club48", "bundle").await;
    for mode in [None, Some("graph")] {
        if let Some(m) = mode {
            set(&p, "dex.router_mode", json!(m)).await;
        }
        for side in [Side::Buy, Side::Sell] {
            for qty in ["1", "1000", "100000"] {
                let qty: Decimal = qty.parse().unwrap();
                let first = route(&p, side, qty).await;
                let s1 = state(&p);
                let again = route(&p, side, qty).await;
                let s2 = state(&p);
                let third = route(&p, side, qty).await;
                assert_eq!(
                    first, again,
                    "{mode:?} {side:?} {qty}: повтор на том же состоянии"
                );
                assert_eq!(again, third, "{mode:?} {side:?} {qty}");
                assert_eq!(
                    s1, s2,
                    "{mode:?} {side:?} {qty}: повторная котировка эпоху не меняет"
                );
            }
        }
    }
}

#[tokio::test]
async fn each_input_changes_state() {
    let p = route_common::parts().await;
    route_common::channel(&p, "club48", "bundle").await;
    set(&p, "dex.router_mode", json!("graph")).await;
    let _ = route(&p, Side::Sell, Decimal::from(1000u32)).await;
    let mut prev = state(&p);
    let mut check = |what: &str, p: &Parts| {
        let now = state(p);
        assert_ne!(prev, now, "{what}: эпоха должна смениться");
        prev = now;
    };
    // Пул монеты и пул промежуточной пары — состояния из записи другого блока (routes_2026-10-30).
    let other = route_common::parts_from("tests/data/routes_2026-10-30").await;
    for addr in [route_common::CAKE_WBNB_PCS, route_common::WBNB_USDT_V2] {
        let r = arb::core::types::PoolRef::Contract(addr);
        let e = other.registry.get(&r).unwrap();
        p.registry.put(e);
        check(&format!("пул {addr}"), &p);
    }
    // Факты проверок пула.
    let r = arb::core::types::PoolRef::Contract(route_common::BTCB_WBNB_PCS);
    p.checks.update_pool(&r, |f| f.gas_sell = Some(130_000));
    check("факты пула", &p);
    // Список промежуточных.
    p.mids.set(vec![Mid {
        address: WBNB,
        ticker: "BNB".into(),
    }]);
    check("промежуточные", &p);
    // Настройка.
    set(&p, "dex.route_paths_ttl_ms", json!(60_000)).await;
    check("настройка", &p);
    // Набор путей монеты в кэше.
    let allowed = HashSet::from([WBNB, BTCB]);
    let s = Search {
        usdt: USDT,
        coin: CAKE,
        allowed: &allowed,
        max_hops: 3,
        max_paths: 50,
        cap: None,
    };
    route_cache::paths(&p.ctx, &p.registry, &p.tokens, 18, &s).unwrap();
    check("набор путей", &p);
}

#[tokio::test]
async fn no_fresh_head_no_epoch_and_counters_grow() {
    let p = route_common::parts().await;
    route_common::channel(&p, "club48", "bundle").await;
    // Головы и цены газа нет (узел не вызывался): эпохи нет — котировка всегда заново.
    assert!(p.gas.known().is_none());
    assert!(signal_epoch::gate(&p).is_none());
    assert!(signal_epoch::of(&p, &cake()).is_none());
    // Поколение итогов проверок.
    let g = p.checks.generation();
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: cake(),
        taxes: CheckState::Known(zero()),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    assert_eq!(p.checks.generation(), g + 1);
    let r = arb::core::types::PoolRef::Contract(route_common::CAKE_WBNB_PCS);
    p.checks.update_pool(&r, |f| f.gas_buy = Some(121_000));
    assert_eq!(p.checks.generation(), g + 2);
    // Поколение настроек.
    let s = p.ctx.settings.generation();
    set(&p, "dex.route_max_paths", json!(40)).await;
    assert_eq!(p.ctx.settings.generation(), s + 1);
}
