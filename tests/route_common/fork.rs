//! Файл: tests/route_common/fork.rs
//! Что делает: сцена решения 271 — синтетическая монета COIN, у которой ЕДИНСТВЕННЫЙ пул COIN-USDT — пул форка
//! (семейство biswap, вид v2_fork) с РЕАЛЬНЫМИ резервами V2-пула записи routes_2026-10-29 (WBNB-USDT v2), как в
//! tests/bsc_dex_biswap_graph.rs: меняется только метка семейства, к сети сцена не ходит. Один бандловый канал и
//! dex.bundle_max_routers — форк-маршрут графа исполним (решения 84, 121 B3). Вынесено из tests/bsc_dex_graph_noroute.rs.
//! Кто вызывает: tests/bsc_dex_graph_noroute.rs, tests/bsc_dex_graph_noroute_gas.rs.
//! routed — выбор маршрута как в Quoting::priced (quote/routing.rs) на этой сцене.
//! От чего зависит: tests/route_common (parts, channel, clean_taxes, USDT), arb::venues::bsc_dex::{parts, pools,
//! quote::routing}.

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolRef, PoolState, Quote, Side, TokenTaxes, VenueId,
};
use arb::gateway::errors::GatewayResult;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::routing;

use super::USDT;

/// Синтетическая монета сцены: её единственный пул — наш форк-пул (как в tests/bsc_dex_biswap_graph.rs).
pub const COIN: Address = address!("0x00000000000000000000000000000000000000c1");
/// Адрес пула сцены.
pub const POOL: Address = address!("0x00000000000000000000000000000000000000c2");
/// Роутер форка biswap сцены (dex.router; к сети не используется).
pub const ROUTER: Address = address!("0x0000000000000000000000000000000000000099");
/// Цена газа сцены, wei.
pub const GAS: u64 = 2_000_000;

/// Нулевые налоги.
pub fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Монета сцены.
pub fn coin() -> Asset {
    Asset {
        ticker: "COIN".into(),
        bsc_contract: Some(COIN),
    }
}

/// Настройка площадки bsc_dex (None — снять).
pub async fn venue(p: &Parts, name: &str, v: Option<Value>) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(name, s, v, "тест").await.unwrap();
}

/// Резервы единственного V2-пула записи routes_2026-10-29 (WBNB-USDT v2). Ошибок нет (нет пула — паника теста).
pub fn real_v2(p: &Parts) -> (U256, U256) {
    p.registry
        .all()
        .into_iter()
        .find_map(|e| match e.pool.state {
            PoolState::V2 { reserve0, reserve1 } => Some((reserve0, reserve1)),
            _ => None,
        })
        .expect("V2-пул в записи routes_2026-10-29")
}

/// Окружение записи с ОДНИМ пулом COIN-USDT семейства v2_fork (реальные резервы V2-пула записи), допущенным к
/// котировке; один бандловый канал и лимит роутеров бандла — форк-маршрут исполним (решения 84, 121 B3).
pub async fn scene() -> Parts {
    let p = setup().await;
    let r = real_v2(&p);
    fork_pool(&p, POOL, COIN, USDT, r);
    p
}

/// Окружение записи без пулов COIN: DEX biswap (v2_fork) с роутером ROUTER, бандловый канал, лимит роутеров бандла,
/// монета COIN (decimals 18, налоги проверены и нулевые). Пулы кладёт fork_pool. Ошибок нет.
pub async fn setup() -> Parts {
    let p = super::parts().await;
    let a = json!(ROUTER.to_string());
    for (n, v) in [
        ("dex.kind", json!("v2_fork")),
        ("dex.pool_enabled", json!(true)),
        ("dex.factory", a.clone()),
        ("dex.router", a.clone()),
        ("dex.v2_fee_source", json!("fixed")),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        let s = Scope::Dex("biswap".into());
        p.ctx.settings.set(n, s, Some(v), "тест").await.unwrap();
    }
    super::channel(&p, "club48", "bundle").await;
    venue(&p, "dex.bundle_max_routers", Some(json!(3))).await;
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        coin(),
    ]);
    p.tokens.remember_decimals(USDT, 18);
    p.tokens.remember_decimals(COIN, 18);
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: coin(),
        taxes: CheckState::Known(zero()),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    p
}

/// Кладёт в реестр пул форка biswap (v2_fork) `addr` «coin — other», допущенный к котировке (газ пробы, сверка с
/// квотером, чистые налоги пула). Резервы — в порядке token0/token1 (сортировка адресов). Ошибок нет.
pub fn fork_pool(p: &Parts, addr: Address, coin: Address, other: Address, r: (U256, U256)) {
    let f = families::make("biswap", "v2_fork").unwrap();
    let (t0, t1) = if coin < other {
        (coin, other)
    } else {
        (other, coin)
    };
    p.registry.put(Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: r.0,
                reserve1: r.1,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: p.tokens.ticker_of(coin).unwrap_or_default(),
        coin,
        usdt: other,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    });
    p.checks.update_pool(&PoolRef::Contract(addr), |f| {
        f.gas_buy = Some(120_000);
        f.gas_sell = Some(120_000);
        f.quoter.buy = Some(Ok(()));
        f.quoter.sell = Some(Ok(()));
        f.taxes = Some(super::clean_taxes());
    });
}

/// Выбор маршрута как в Quoting::priced (quote/routing.rs): пулы монеты с USDT из реестра, активные промежуточные,
/// 1000 COIN. Принимает: окружение, сторону, умный ли режим классики. Возвращает: как routing::run.
pub async fn routed(p: &Parts, side: Side, smart: bool) -> GatewayResult<Quote> {
    let q = p.quoting();
    let mids = q
        .mids
        .active(&q.ctx)
        .into_iter()
        .map(|m| (m, 18u8))
        .collect();
    let input = routing::Input {
        asset: coin(),
        side,
        qty: Decimal::from(1000u32),
        coin: COIN,
        usdt: USDT,
        taxes: zero(),
        coin_dec: 18,
        usdt_dec: 18,
        entries: q.reg.by_pair(COIN, USDT),
        mids,
        gas: U256::from(GAS),
        usdt_ticker: "USDT".into(),
        smart,
    };
    routing::run(q, input).await
}
