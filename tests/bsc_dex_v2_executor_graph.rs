//! Файл: tests/bsc_dex_v2_executor_graph.rs
//! Что делает: решение 280 — v2_fork_executor на РЕАЛЬНЫХ резервах V2-пула записи routes_2026-10-29 (меняется только
//! метка семейства, без сети): без исполнителя маршрут — сигнал с понятной причиной (не «dex.router»), с исполнителем —
//! исполним; записанный откат покупки («DAOSwap: K», блок 133619188) у нового вида — «налоги пула не измерены», у
//! v2_fork — прежний вердикт по монете. Кто вызывает: cargo test. От чего зависит: tests/route_common, arb::venues::bsc_dex.

mod route_common;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{
    Asset, CheckState, CoinCheck, DexRoute, Pool, PoolRef, PoolState, Side, TokenTaxes, VenueId,
};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::{probe_calls, simulate::CallResult};
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::v2_pair_probe::buy_unmeasured;
use arb::venues::bsc_dex::quote::{graph_route, steps::Names, via::Via};
use route_common::USDT;

/// Синтетическая монета и адрес пула теста (путь COIN↔USDT — только через наш пул).
const COIN: Address = address!("0x00000000000000000000000000000000000000c1");
const POOL: Address = address!("0x00000000000000000000000000000000000000c2");

/// Нулевые налоги монеты.
const ZERO: TokenTaxes = TokenTaxes {
    buy_pct: Decimal::ZERO,
    sell_pct: Decimal::ZERO,
    transfer_pct: Decimal::ZERO,
};

/// Пул COIN-USDT вида `kind` с реальными резервами V2-пула записи. Ошибок нет.
fn coin_usdt(p: &Parts, id: &str, kind: &str) -> Entry {
    let (r0, r1) = p
        .registry
        .all()
        .into_iter()
        .find_map(|e| match e.pool.state {
            PoolState::V2 { reserve0, reserve1 } => Some((reserve0, reserve1)),
            _ => None,
        })
        .expect("V2-пул в записи routes_2026-10-29");
    let f = families::make(id, kind).unwrap();
    let (t0, t1) = if COIN < USDT {
        (COIN, USDT)
    } else {
        (USDT, COIN)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(POOL),
            token0: t0,
            token1: t1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: r0,
                reserve1: r1,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "COIN".into(),
        coin: COIN,
        usdt: USDT,
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

/// Окружение записи с пулом нового вида: фабрика и комиссия, БЕЗ dex.router; пул допущен к котировке.
async fn scene() -> Parts {
    let p = route_common::parts().await;
    let s = Scope::Dex("dao_swap".into());
    for (n, v) in [
        ("dex.kind", json!("v2_fork_executor")),
        ("dex.pool_enabled", json!(true)),
        (
            "dex.factory",
            json!("0x940BEb635cbEeC04720AC97FADb97205676e6aa4"),
        ),
        ("dex.v2_fee_source", json!("fixed")),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        p.ctx
            .settings
            .set(n, s.clone(), Some(v), "тест")
            .await
            .unwrap();
    }
    let asset = |t: &str, a: Address| Asset {
        chain: None,
        ticker: t.into(),
        contract: Some(a),
    };
    p.tokens.set(&[asset("USDT", USDT), asset("COIN", COIN)]);
    p.tokens.remember_decimals(USDT, 18);
    p.tokens.remember_decimals(COIN, 18);
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: asset("COIN", COIN),
        taxes: CheckState::Known(ZERO),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    p.registry
        .put(coin_usdt(&p, "dao_swap", "v2_fork_executor"));
    p.checks.update_pool(&PoolRef::Contract(POOL), |f| {
        f.gas_buy = Some(120_000);
        f.quoter.buy = Some(Ok(()));
        f.taxes = Some(route_common::clean_taxes());
    });
    p
}

/// Котирует покупку COIN графом. Ошибок нет.
async fn quote_route(p: &Parts) -> DexRoute {
    let q = p.quoting();
    let v = Via {
        q: &q,
        coin: COIN,
        usdt: USDT,
        side: Side::Buy,
        taxes: &ZERO,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "COIN"),
        mids: &p.mids,
        gas: "BNB",
    };
    let asset = Asset {
        chain: None,
        ticker: "COIN".into(),
        contract: Some(COIN),
    };
    let (qty, gas) = (Decimal::from(1000u32), U256::from(2_000_000u64));
    let quote = graph_route::quote(&v, &names, &asset, qty, gas).await;
    quote
        .expect("котировка графа")
        .route
        .expect("маршрут графа")
}

/// Без исполнителя — сигнал с понятной причиной; с исполнителем у кошелька — исполним.
#[tokio::test]
async fn executable_only_with_executor() {
    let p = scene().await;
    let r = quote_route(&p).await;
    assert!(!r.executable, "без исполнителя — сигнал");
    let note = r.exec_note.unwrap_or_default();
    assert!(note.contains("без своего пути исполнения"), "{note}");
    assert!(!note.contains("dex.router"), "{note}");
    route_common::executor(&p, "0x00000000000000000000000000000000000000ee").await;
    let r = quote_route(&p).await;
    assert!(r.executable, "с исполнителем — исполним: {:?}", r.exec_note);
}

/// Откат покупки пробы: у v2_fork_executor — «налоги пула не измерены», у v2_fork — вердикт по монете, как раньше.
#[tokio::test]
async fn buy_revert_is_pool_property_only_for_new_kind() {
    let p = route_common::parts().await;
    let path = "tests/data/v2_fork_executor_2026-11-15/pairs.json";
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let r: CallResult =
        serde_json::from_value(rec["dex"]["dao_swap"]["below_1pip_last"].clone()).unwrap();
    let new = coin_usdt(&p, "dao_swap", "v2_fork_executor");
    // Номера как в пробе: «ожидалось» — 3, покупка — 5.
    let why = buy_unmeasured(&new, 5, 3, &r).expect("свойство пула");
    assert!(
        why.contains("налоги пула не измерены") && why.contains("DAOSwap: K"),
        "{why}"
    );
    assert!(
        buy_unmeasured(&new, 1, 3, &r).is_none(),
        "до «ожидалось» — как у всех"
    );
    assert!(buy_unmeasured(&coin_usdt(&p, "biswap", "v2_fork"), 5, 3, &r).is_none());
    assert!(
        probe_calls::buy_revert(&p.ctx, 5, &r, 3).is_ok(),
        "v2_fork: вердикт по монете"
    );
}
