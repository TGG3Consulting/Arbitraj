//! Файл: tests/bsc_dex_verify.rs
//! Что делает: предторговая верификация маршрута (quote/verify.rs, решение 83, фазы 80.6a/80.6b) на реальных записях
//! CAKE. Ok: свежее состояние = тот же блок плана (routes_2026-10-29) → пересчёт совпадает → drift 0 → Ok, с
//! amountOutMin по каждой ноге (решение 84). Reject: план на routes_2026-10-29, свежее состояние на routes_2026-10-30
//! (более поздний блок 130556914, цена CAKE уехала ~5.9%) → drift > verify.max_price_drift_pct (в тесте 1%) → Reject с
//! дрейфом. Плюс чтение пределов verify.* из настроек, перевод вердикта в решение шлюза (exec/verify_gate.rs outcome:
//! Ok → свежие минимумы, Reject → ошибка, ордер не формируется) и восстановление цены плана из худшей допустимой.
//! Без синтетики — оба состояния из записанных ответов RPC.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{verify, graph_route, via, steps}, exec::verify_gate, tests/route_common.

mod route_common;

use std::collections::HashMap;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{
    Asset, DexRoute, PoolRef, RouteMode, Side, TokenTaxes, TradeRequest, VenueId,
};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::verify_gate;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::verify::{self, Verdict};
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, graph_route};

use route_common::{CAKE, USDT};

/// Допуск проскальзывания для amountOutMin в тестах (exec.dex_slippage_pct = 0.5%).
fn slip() -> Decimal {
    Decimal::new(5, 1)
}

const GAS: u64 = 1_000_000_000;
/// Блок записи routes_2026-10-29 (0x7c79b28).
const BLOCK: u64 = 130_521_896;
/// Блок записи routes_2026-10-30 (0x7c823f2) — свежее состояние, цена CAKE уехала.
const BLOCK_30: u64 = 130_556_914;

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    }
}

fn via<'a>(q: &'a Quoting, t: &'a TokenTaxes) -> Via<'a> {
    Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: t,
        coin_dec: 18,
        usdt_dec: 18,
    }
}

fn names<'a>(q: &'a Quoting) -> Names<'a> {
    Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    }
}

#[tokio::test]
async fn decide_ok_when_state_unchanged() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let plan = graph_route::quote(
        &via(&q, &t),
        &names(&q),
        &cake(),
        Decimal::from(50_000u32),
        U256::from(GAS),
    )
    .await
    .expect("план-котировка графа");
    let route = plan.route.as_ref().unwrap();
    // Свежее состояние = текущие записи реестра (тот же блок записи) → пересчёт совпадает, дрейф 0.
    let fresh: HashMap<PoolRef, Entry> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| (h.pool.clone(), q.reg.get(&h.pool).expect("пул в реестре")))
        .collect();
    let verdict = verify::decide(
        &via(&q, &t),
        &names(&q),
        &plan,
        &fresh,
        U256::from(GAS),
        BLOCK,
        Decimal::ONE,       // допуск дрейфа 1%
        Decimal::new(5, 1), // exec.dex_slippage_pct = 0.5% для amountOutMin (решение 84)
    );
    match verdict {
        Verdict::Ok {
            amount_out_min,
            block,
            ..
        } => {
            assert_eq!(block, BLOCK);
            assert_eq!(
                amount_out_min.len(),
                route.legs.len(),
                "amountOutMin по каждой ноге"
            );
            assert!(amount_out_min.iter().all(|m| *m > Decimal::ZERO));
        }
        Verdict::Reject { reason, drift } => {
            panic!(
                "ожидался Ok при неизменном состоянии, получен Reject: {reason} (drift {drift:?})"
            )
        }
    }
}

#[tokio::test]
async fn limits_read_from_settings() {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("verify.max_staleness_blocks", serde_json::json!(2)),
        ("verify.max_price_drift_pct", serde_json::json!("0.5")),
        ("verify.timeout_ms", serde_json::json!(800)),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    assert_eq!(
        verify::limits(&p.ctx).unwrap(),
        (2, Decimal::new(5, 1), 800)
    );
}

#[tokio::test]
async fn reject_on_price_drift() {
    // План считаем на записи 2026-10-29, свежее состояние берём на записи 2026-10-30 (тот же record_route.js, те же
    // 6 пулов CAKE, более поздний блок, где цена CAKE уехала ~5.9%). Пересчёт нашей математикой на свежем состоянии
    // даёт avg_price, уехавшую от плана больше допуска 1% → Verdict::Reject с конкретным дрейфом.
    let plan_p = route_common::parts().await;
    let plan_q = plan_p.quoting();
    let t = zero();
    let plan = graph_route::quote(
        &via(&plan_q, &t),
        &names(&plan_q),
        &cake(),
        Decimal::from(1000u32),
        U256::from(GAS),
    )
    .await
    .expect("план-котировка графа (2026-10-29)");
    let route = plan.route.as_ref().unwrap();
    // Свежее состояние — из второй записи: те же пулы маршрута, но на более позднем блоке.
    let fresh_p = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let fresh_q = fresh_p.quoting();
    let fresh: HashMap<PoolRef, Entry> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| {
            (
                h.pool.clone(),
                fresh_q
                    .reg
                    .get(&h.pool)
                    .expect("пул маршрута в свежей записи"),
            )
        })
        .collect();
    // Пересчёт и вердикт — на окружении свежей записи (глубина/допуск пула от свежего состояния); план — с первой.
    let verdict = verify::decide(
        &via(&fresh_q, &t),
        &names(&fresh_q),
        &plan,
        &fresh,
        U256::from(GAS),
        BLOCK_30,
        Decimal::ONE,       // допуск verify.max_price_drift_pct = 1%
        Decimal::new(5, 1), // exec.dex_slippage_pct = 0.5%
    );
    match verdict {
        Verdict::Reject { reason, drift } => {
            let drift = drift.expect("дрейф посчитан");
            assert!(
                drift > Decimal::ONE,
                "дрейф {drift}% должен превышать допуск 1%: {reason}"
            );
        }
        Verdict::Ok { .. } => panic!("ожидался Reject: цена CAKE уехала между блоками"),
    }
}

#[tokio::test]
async fn gate_outcome_passes_fresh_blocks_drift() {
    // Перевод вердикта в решение шлюза (exec/verify_gate.rs outcome): Ok пропускает со свежими amountOutMin,
    // Reject (цена уехала) блокирует сделку — ордер не формируется (ошибка QuoteMismatch).
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let plan = graph_route::quote(
        &via(&q, &t),
        &names(&q),
        &cake(),
        Decimal::from(1000u32),
        U256::from(GAS),
    )
    .await
    .expect("план-котировка графа");
    let route = plan.route.as_ref().unwrap();
    // Ok: свежее = блок плана (routes_2026-10-29) → decide Ok → outcome Ok(Verified) со свежими минимумами.
    let fresh_ok: HashMap<PoolRef, Entry> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| (h.pool.clone(), q.reg.get(&h.pool).expect("пул в реестре")))
        .collect();
    let ok = verify::decide(
        &via(&q, &t),
        &names(&q),
        &plan,
        &fresh_ok,
        U256::from(GAS),
        BLOCK,
        Decimal::ONE,
        slip(),
    );
    let verified = verify_gate::outcome(&p.ctx, ok).expect("Ok пропускает сделку");
    assert_eq!(verified.block, BLOCK);
    assert_eq!(verified.amount_out_min.len(), route.legs.len());
    assert!(verified.amount_out_min.iter().all(|m| *m > Decimal::ZERO));

    // Reject: свежее = routes_2026-10-30 (цена уехала) → decide Reject → outcome Err(QuoteMismatch): ордер не формируется.
    let fp = route_common::parts_from("tests/data/routes_2026-10-30").await;
    let fq = fp.quoting();
    let fresh_drift: HashMap<PoolRef, Entry> = route
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| {
            (
                h.pool.clone(),
                fq.reg.get(&h.pool).expect("пул маршрута в свежей записи"),
            )
        })
        .collect();
    let rej = verify::decide(
        &via(&fq, &t),
        &names(&fq),
        &plan,
        &fresh_drift,
        U256::from(GAS),
        BLOCK_30,
        Decimal::ONE,
        slip(),
    );
    let err = verify_gate::outcome(&fp.ctx, rej).expect_err("Reject блокирует сделку");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
}

#[tokio::test]
async fn gate_outcome_stale_maps_to_stale_data() {
    // Reject без дрейфа (устаревшее состояние / сбой чтения / таймаут) → StaleData.
    let p = route_common::parts().await;
    let verdict = Verdict::Reject {
        reason: "состояние устарело: блок отстал от головы".into(),
        drift: None,
    };
    let err = verify_gate::outcome(&p.ctx, verdict).expect_err("Reject блокирует сделку");
    assert_eq!(err.code, ErrorCode::StaleData);
}

#[tokio::test]
async fn plan_price_reconstructed_from_worst() {
    // Средняя цена плана восстанавливается из худшей допустимой обратной формулой core/worst.rs
    // (покупка: worst = avg × (1 + slip)); avg = 2.0, slip = 0.5% → worst = 2.01 → назад ровно 2.0.
    let p = route_common::parts().await;
    let route = DexRoute {
        mode: RouteMode::Graph,
        legs: Vec::new(),
        block: 1,
        gas_units: 0,
        executable: true,
        exec_note: None,
    };
    let req = TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: cake(),
        side: Side::Buy,
        qty: Decimal::from(1000u32),
        worst_price: Decimal::new(201, 2), // 2.01
        deal_id: "t".into(),
        route: Some(route.clone()),
    };
    let plan =
        verify_gate::plan_quote(&p.ctx, &req, &route, slip()).expect("цена плана восстановлена");
    assert!(
        (plan.avg_price - Decimal::from(2u32)).abs() < Decimal::new(1, 9),
        "avg_price = {} (ожидалось 2.0)",
        plan.avg_price
    );
}
