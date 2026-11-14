//! Файл: tests/bsc_dex_bundle_no_path.rs
//! Что делает: ревью решения 280 — нога бандла у DEX без своего пути исполнения (family.tradable = нет:
//! v2_fork_executor, v3_fork, kyber_elastic — исполняются только шагом исполнителя) получает отказ NoRoute с точной
//! причиной ДО чтения dex.router (exec/bundle.rs fork_router): даже если dex.router задан (остался от прежнего вида
//! v2_fork или введён по ошибке), свопа через этот роутер нет. Нога v2_fork (tradable = да) с тем же роутером
//! собирается, как раньше. Состояние пула — РЕАЛЬНЫЕ резервы V2-пула записи routes_2026-10-29 (меняется только метка
//! семейства); к сети тест не ходит.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle, chain, plan}, pools::families, tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256, address};
use serde_json::json;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::{self, BundleTx};
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::Quoting;

use route_common::{USDT, WBNB, WBNB_USDT_V2};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");
const ROUTER: Address = address!("0x00000000000000000000000000000000000000b1");

/// Реальный V2-пул записи с меткой семейства (id DEX, вид). Ошибок нет.
fn relabeled(q: &Quoting, id: &str, kind: &str) -> Entry {
    let mut e = q
        .reg
        .get(&PoolRef::Contract(WBNB_USDT_V2))
        .expect("пул в реестре");
    let f = families::make(id, kind).unwrap();
    e.family = f;
    e.pool.family = f.id.into();
    e
}

/// Нога из одного шага USDT → WBNB (вход с кошелька, выход на кошелёк, минимум не 0).
fn leg(e: Entry) -> Vec<Step> {
    vec![Step {
        entry: e,
        order: Order {
            token_in: USDT,
            token_out: WBNB,
            amount_in: U256::from(1_000_000u64),
            min_out: U256::from(1u8),
        },
        from_wallet: true,
        to_wallet: true,
    }]
}

/// Срок, разрешения и dex.router у обеих DEX (у dao_swap — «оставшийся» от прежнего вида).
async fn prepare(p: &arb::venues::bsc_dex::parts::Parts) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.deadline_s", json!(60)),
        ("dex.approve_unlimited", json!(false)),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    for id in ["dao_swap", "biswap", "sheepdex"] {
        let s = Scope::Dex(id.into());
        let r = json!(format!("{ROUTER}"));
        p.ctx
            .settings
            .set("dex.router", s, Some(r), "тест")
            .await
            .unwrap();
    }
}

/// Нога без своего пути исполнения — отказ с причиной, свопа через dex.router нет; v2_fork — своп, как раньше.
#[tokio::test]
async fn untradable_leg_refused_before_router() {
    let p = route_common::parts().await;
    prepare(&p).await;
    let q = p.quoting();
    for (id, kind) in [("dao_swap", "v2_fork_executor"), ("sheepdex", "v3_fork")] {
        let e = relabeled(&q, id, kind);
        assert!(
            e.family.fork && !e.family.tradable,
            "{kind}: форк без своего пути"
        );
        let err = bundle::build(&q.ctx, &[leg(e)], WBNB, WALLET, 7, 1_000_000, true)
            .await
            .expect_err("нога без своего пути исполнения — отказ");
        assert_eq!(err.code, ErrorCode::NoRoute, "{kind}");
        assert!(
            err.message.contains("нет своего пути исполнения")
                && err.message.contains("нужен исполнитель у кошелька"),
            "{kind}: {}",
            err.message
        );
    }
    let e = relabeled(&q, "biswap", "v2_fork");
    let b = bundle::build(&q.ctx, &[leg(e)], WBNB, WALLET, 7, 1_000_000, true)
        .await
        .expect("v2_fork — как раньше");
    assert!(
        b.txs
            .iter()
            .any(|t| matches!(t, BundleTx::ForkSwap { router, .. } if *router == ROUTER)),
        "v2_fork: своп своим роутером"
    );
}
