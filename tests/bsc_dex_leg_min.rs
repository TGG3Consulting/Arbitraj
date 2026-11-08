//! Файл: tests/bsc_dex_leg_min.rs
//! Что делает: решение 228 — минимум выхода доли min_k = E_k × max(1 − exec.dex_slippage_pct, B / E)
//! (exec/leg_min.rs set_leg_mins) на РЕАЛЬНОЙ записи CAKE 2026-10-29 (tests/route_common, блок 130521896), до wei:
//! (а) продажа 40 + 60 WBNB через ОДИН пул V2 WBNB-USDT по порядку (решение 223): доля 2 честно дешевле средней —
//!     старый минимум «60 × worst» выше её ожидаемого (UR/бандл — откат наверняка), новый — ниже; Σ = B (+1 wei вверх);
//!     при малом допуске старая Σ max(…) выше ожидаемого всей сделки (minOut исполнителя — откат), новая — ниже;
//!     minOut вызова исполнителя (call.rs) = Σ новых минимумов;
//! (б) покупка 40 + 60 WBNB за USDT через тот же пул: то же по монетам (B = Σ USDT / worst);
//! (в) одна доля (продажа и покупка, обе ветки max) — побайтно как старый минимум exec/order.rs;
//! (г) минимум верификации: ниже — не опускает, строже — ставится, выше ожидаемого доли — отказ до отправки;
//!     граница сделки выше ожидаемого — отказ.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::{chain, leg_min, executor::call}, quote::{graph_seq, graph_legs,
//! select, route_calc}, num}, tests/route_common.

mod route_common;

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, PoolRef, Side, TokenTaxes, TradeRequest, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::chain::{self, Step};
use arb::venues::bsc_dex::exec::executor::{call, sim};
use arb::venues::bsc_dex::exec::leg_min::set_leg_mins;
use arb::venues::bsc_dex::num;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::path::Path;
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::{Quoting, graph_legs, graph_seq, select};

use route_common::{USDT, WBNB, WBNB_USDT_V2};

const DECS: (u8, u8) = (18, 18);

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn u(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

fn units(x: Decimal) -> U256 {
    num::to_units(x, 18).unwrap()
}

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

async fn settings(p: &Parts, slip: &str) {
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("exec.dex_slippage_pct", json!(slip)),
        ("dex.deadline_s", json!(60)),
    ] {
        p.ctx
            .settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
}

/// Нога через записанный пул V2 WBNB-USDT от `tin` к `tout`.
fn leg(q: &Quoting, tin: Address, tout: Address) -> Leg {
    let e = q
        .reg
        .get(&PoolRef::Contract(WBNB_USDT_V2))
        .expect("пул записи");
    let side = graph_legs::hop_side(&e, tin).expect("ребро");
    let usable = select::usable(q, &e, side, 18).expect("пул допущен");
    Leg {
        entry: e,
        usable,
        token_in: tin,
        token_out: tout,
        dec_in: 18,
        dec_out: 18,
    }
}

fn req(side: Side, qty: Decimal, worst: Decimal) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "BNB".into(),
            bsc_contract: Some(WBNB),
        },
        side,
        qty,
        worst_price: worst,
        deal_id: "t".into(),
        route: None,
        exit_position: false,
    }
}

/// Доли по порядку (решение 223) через один пул: (путь, монет доли).
fn shares(q: &Quoting, side: Side, qs: &[Decimal]) -> Vec<(Path, Decimal)> {
    let l = if side == Side::Sell {
        leg(q, WBNB, USDT)
    } else {
        leg(q, USDT, WBNB)
    };
    let one = std::slice::from_ref(&l);
    let input: Vec<(&[Leg], Decimal)> = qs.iter().map(|x| (one, *x)).collect();
    let paths = graph_seq::eval(&q.ctx, &input, side, &zero()).expect("доли считаются");
    paths.into_iter().zip(qs.iter().copied()).collect()
}

/// Шаги долей (старый предварительный минимум exec/order.rs в последнем шаге) и минимумы решения 228.
fn build(
    q: &Quoting,
    r: &TradeRequest,
    paths: &[(Path, Decimal)],
    ver: Option<&[Decimal]>,
) -> (Vec<U256>, Result<Legs, arb::gateway::errors::GatewayError>) {
    let mut legs: Vec<Vec<Step>> = paths
        .iter()
        .map(|(p, qty)| chain::steps(&q.ctx, r, p, *qty, DECS).expect("шаги"))
        .collect();
    let old = legs
        .iter()
        .map(|l| l.last().unwrap().order.min_out)
        .collect();
    let shares = paths.iter().map(|(p, qty)| (p, *qty));
    let res = set_leg_mins(&q.ctx, r, shares, &mut legs, ver, DECS).map(|_| Legs(legs));
    (old, res)
}

/// Шаги долей после set_leg_mins (обёртка: Step без Debug, а expect_err требует Debug у Ok).
struct Legs(Vec<Vec<Step>>);

impl std::fmt::Debug for Legs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "минимумы {:?}", mins(&self.0))
    }
}

fn mins(legs: &[Vec<Step>]) -> Vec<U256> {
    legs.iter()
        .map(|l| l.last().unwrap().order.min_out)
        .collect()
}

#[tokio::test]
async fn sell_shared_v2_late_share_min_not_above_expected() {
    let p = route_common::parts().await;
    settings(&p, "0.5").await;
    let q = p.quoting();
    let paths = shares(&q, Side::Sell, &[d("40"), d("60")]);
    let e: Vec<U256> = paths.iter().map(|(p, _)| units(p.usdt)).collect();
    // Записанные выходы долей по порядку (решение 223, tests/bsc_dex_graph_shared.rs): цены 741.511… и 738.779…
    assert_eq!(
        e,
        [u("29660451317560444250794"), u("44326781707196577861001")]
    );
    let r = req(Side::Sell, d("100"), d("739.5"));
    let (old, new) = build(&q, &r, &paths, None);
    let new = mins(&new.expect("минимумы").0);
    // Старый минимум доли 2: 60 × 739.5 = 44370 USDT > её ожидаемого 44326.78… — откат ноги наверняка.
    assert_eq!(old[1], u("44370000000000000000000"));
    assert!(old[1] > e[1]);
    // Новый: B = 100 × 739.5 = 73950 ≥ E × 0.995 → min_k = ceil(B_u × E_k_u / E_u).
    assert_eq!(
        new,
        [u("29645525116470566368146"), u("44304474883529433631855")]
    );
    assert!(
        new[0] <= e[0] && new[1] <= e[1],
        "ни одна доля не выше своего ожидаемого"
    );
    assert_eq!(
        new[0] + new[1],
        u("73950000000000000000001"),
        "Σ = B (вверх на 1 wei)"
    );
}

#[tokio::test]
async fn sell_small_slippage_executor_sum_not_above_expected() {
    let p = route_common::parts().await;
    settings(&p, "0.05").await;
    let q = p.quoting();
    let paths = shares(&q, Side::Sell, &[d("40"), d("60")]);
    let total = units(paths[0].0.usdt) + units(paths[1].0.usdt);
    let r = req(Side::Sell, d("100"), d("739.5"));
    let (old, new) = build(&q, &r, &paths, None);
    let legs = new.expect("минимумы").0;
    // Старая Σ max(qty_k × worst, E_k × 0.9995) = 29645.62… + 44370 = 74015.62… > E = 73987.23… — minOut откат.
    assert_eq!(old[0] + old[1], u("74015621091901664028668"));
    assert!(old[0] + old[1] > total);
    // Новая: E × 0.9995 = 73950.239… > B = 73950 → min_k = to_units(E_k × 0.9995).
    assert_eq!(
        mins(&legs),
        [u("29645621091901664028668"), u("44304618316342979572070")]
    );
    let c = call::build(&q.ctx, &legs, Address::repeat_byte(0xee), 1_000_000).expect("вызов");
    assert_eq!(
        c.min_out,
        u("73950239408244643600738"),
        "minOut исполнителя = Σ минимумов долей"
    );
    assert!(c.min_out <= total && c.min_out >= units(d("73950")));
}

#[tokio::test]
async fn buy_shared_v2_late_share_min_not_above_expected() {
    let p = route_common::parts().await;
    settings(&p, "0.5").await;
    let q = p.quoting();
    let paths = shares(&q, Side::Buy, &[d("40"), d("60")]);
    // Записанные доли по порядку: 40 WBNB за 29897.638800016430506083 USDT (747.44…), 60 — за 45013.308362478985899041
    // (750.22…; поздняя доля дороже средней 749.109…).
    let got: Vec<(Decimal, Decimal)> = paths.iter().map(|(p, _)| (p.coins, p.usdt)).collect();
    assert_eq!(
        got,
        [
            (d("40"), d("29897.638800016430506083")),
            (d("60"), d("45013.308362478985899041"))
        ]
    );
    // Худшая цена 749.5: средняя её не хуже (check_price), доля 2 — хуже.
    let r = req(Side::Buy, d("100"), d("749.5"));
    let (old, new) = build(&q, &r, &paths, None);
    let new = mins(&new.expect("минимумы").0);
    // Старый минимум доли 2: 45013.30… / 749.5 = 60.0577… WBNB > её 60 — откат ноги наверняка.
    assert_eq!(old[1], u("60057783005308853767"));
    // Новый: B = 74910.947162495416405124 / 749.5 = 99.947894813202690333… ≥ 100 × 0.995 → ceil(B_u × E_k_u / E_u).
    assert_eq!(new, [u("39979157925281076134"), u("59968736887921614200")]);
    assert!(new[0] <= units(d("40")) && new[1] <= units(d("60")));
    assert_eq!(
        new[0] + new[1],
        u("99947894813202690334"),
        "Σ = B_u + 1 wei"
    );
}

/// Одна доля: новый минимум побайтно равен старому exec/order.rs (предварительный минимум chain::steps) — продажа и
/// покупка, ветка «граница сделки» и ветка «допуск».
#[tokio::test]
async fn single_share_same_as_before_bytewise() {
    let p = route_common::parts().await;
    settings(&p, "0.5").await;
    let q = p.quoting();
    for side in [Side::Sell, Side::Buy] {
        let paths = shares(&q, side, &[d("40")]);
        let avg = paths[0].0.usdt / paths[0].0.coins;
        // Худшая цена: 0.1% от средней (ветка B) и 1% (ветка допуска 0.5%) — в сторону, допустимую check_price.
        for pct in [d("0.001"), d("0.01")] {
            let worst = if side == Side::Sell {
                avg * (Decimal::ONE - pct)
            } else {
                avg * (Decimal::ONE + pct)
            };
            let r = req(side, d("40"), worst);
            let (old, new) = build(&q, &r, &paths, None);
            let new = mins(&new.expect("минимум").0);
            println!("{side:?} worst {worst}: было {} стало {}", old[0], new[0]);
            assert_eq!(new, old, "одна доля — как раньше, до wei");
        }
    }
}

/// Минимум верификации: ниже — не опускает минимум, строже (но не выше ожидаемого доли) — ставится, выше ожидаемого
/// доли — отказ до отправки; число минимумов не равно долям — отказ; граница сделки выше ожидаемого — отказ.
#[tokio::test]
async fn verification_min_is_floor_not_above_expected() {
    let p = route_common::parts().await;
    settings(&p, "0.5").await;
    let q = p.quoting();
    let paths = shares(&q, Side::Sell, &[d("40"), d("60")]);
    let r = req(Side::Sell, d("100"), d("739.5"));
    let e2 = paths[1].0.usdt;
    let base = [u("29645525116470566368146"), u("44304474883529433631855")];
    // Ниже формулы — минимумы те же.
    let low = [d("29000"), d("44000")];
    let (_, got) = build(&q, &r, &paths, Some(&low));
    assert_eq!(mins(&got.unwrap().0), base);
    // Строже формулы, ровно ожидаемое доли 2 — ставится (до wei).
    let strict = [d("29000"), e2];
    let (_, got) = build(&q, &r, &paths, Some(&strict));
    assert_eq!(mins(&got.unwrap().0), [base[0], units(e2)]);
    // Выше ожидаемого доли 2 на 1 wei — честный отказ до отправки.
    let above = [d("29000"), e2 + d("0.000000000000000001")];
    let err = build(&q, &r, &paths, Some(&above))
        .1
        .expect_err("выше ожидаемого");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
    assert!(err.message.contains("доля 2"), "{}", err.message);
    // Число минимумов не равно долям.
    let err = build(&q, &r, &paths, Some(&low[..1]))
        .1
        .expect_err("число ног");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
    // Граница сделки выше ожидаемого всей сделки (worst 739.9 > средней 739.87…) — отказ.
    let r = req(Side::Sell, d("100"), d("739.9"));
    let err = build(&q, &r, &paths, None).1.expect_err("B > E");
    assert_eq!(err.code, ErrorCode::QuoteMismatch);
}

/// Решение 234 (п. 4): текст отката пробы исполнителя — minOut и ожидаемое каждой доли, модельный налог монеты
/// (exec/executor/sim.rs legs_text, explain) — на тех же записанных долях 40 + 60 WBNB, что выше.
#[tokio::test]
async fn revert_text_has_min_expected_tax_per_leg() {
    let p = route_common::parts().await;
    settings(&p, "0.5").await;
    let q = p.quoting();
    let paths = shares(&q, Side::Sell, &[d("40"), d("60")]);
    let r = req(Side::Sell, d("100"), d("739.5"));
    let legs = build(&q, &r, &paths, None).1.expect("минимумы").0;
    let exp: Vec<(Decimal, Decimal)> = paths.iter().map(|(p, _)| (p.usdt, Decimal::ZERO)).collect();
    let text = sim::legs_text(&legs, &exp, &r, DECS);
    assert_eq!(
        text,
        "доля 1: minOut 29645.525116470566368146 USDT, ожидалось 29660.451318 USDT, модельный налог монеты 0%; \
         доля 2: minOut 44304.474883529433631855 USDT, ожидалось 44326.781707 USDT, модельный налог монеты 0%"
    );
    let e = arb::gateway::errors::GatewayError::new(
        ErrorCode::NoRoute,
        None,
        "проба исполнителя откатилась — minOut",
    );
    let e = sim::explain(e, &text);
    assert_eq!(e.code, ErrorCode::NoRoute);
    assert!(
        e.message
            .starts_with("проба исполнителя откатилась — minOut; доля 1: minOut 29645.525116"),
        "{}",
        e.message
    );
}
