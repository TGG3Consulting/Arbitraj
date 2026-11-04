//! Файл: tests/bsc_dex_multihop.rs
//! Что делает: решение 122, часть R2 — нога из N шагов одним Universal Router — на пулах РЕАЛЬНОЙ записи CAKE
//! (tests/route_common, routes_2026-10-29): путь глубины 3 (USDT → WBNB → BTCB → CAKE) из графа пересчитывается живым
//! путём exec/live_hops.rs той же математикой, что котировка; шаги исполнения (exec/chain.rs) — первый платит кошелёк
//! суммой сделки, средние — роутер всем балансом (CONTRACT_BALANCE) роутеру же (ADDRESS_THIS), последний — кошельку
//! с минимумом сделки; входы команд V2/V3 кодируются по Dispatcher.sol обоих UR; средний шаг V4 (SETTLE из баланса
//! роутера → обмен → TAKE роутеру) больше не отвергается; исполнимость графа: нога из 3 хопов одним UR — исполнима,
//! длиннее dex.route_max_hops — нет; монета с налогом через промежуточную с пулом V3 — неисполнима и в котировке графа
//! (решение 39); ревью 122, пункт 3 — монета с налогом на входе через исполнитель: только одна доля с первым шагом
//! «вход переводом в пул» (V2/SOLIDLY), иначе (V3 первым, сплит) — сигнал с причиной.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::{chain, live_hops, swap_v4, ur}, quote::{graph_exec, graph_legs,
//! route_calc, route_paths, graph_executor}, pools::{graph, families}}, tests/route_common.

mod route_common;

use std::collections::{HashMap, HashSet};

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{
    Asset, CheckState, CoinCheck, PoolKind, PoolRef, Side, TokenTaxes, TradeRequest, VenueId,
};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::chain;
use arb::venues::bsc_dex::exec::live_hops;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::exec::{swap_v4, ur};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::key::{PoolKey, UniswapPoolKeyData};
use arb::venues::bsc_dex::quote::path::Path;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{
    graph_exec, graph_executor, graph_legs, route_calc, route_paths,
};

use route_common::{BTCB, CAKE, USDT, WBNB};

const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

/// Налоги промежуточных монет проверены и нулевые (условие решения 52 для внутренних токенов пути).
fn mids_checked(p: &arb::venues::bsc_dex::parts::Parts) {
    for (t, a) in [("BNB", WBNB), ("BTCB", BTCB)] {
        p.checks.set_coin(CoinCheck {
            venue: VenueId("bsc_dex".into()),
            asset: Asset {
                ticker: t.into(),
                bsc_contract: Some(a),
            },
            taxes: CheckState::Known(zero()),
            source: "тест".into(),
            checked_at: Some(Utc::now()),
            wait: None,
        });
    }
}

/// Путь глубины 3 из записи, оценённый на 1 CAKE (покупка; на записи пул CAKE-BTCB узкий — объём малый).
fn three_hop(q: &arb::venues::bsc_dex::quote::Quoting) -> Path {
    let g = graph::build(&q.ctx, &q.reg, &q.tokens, 18).expect("граф");
    let allowed: HashSet<Address> = [WBNB, BTCB].into_iter().collect();
    let paths = route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 50);
    let three = paths
        .iter()
        .find(|p| p.hops() == 3)
        .expect("путь глубины 3");
    let dec: HashMap<Address, u8> = [CAKE, USDT, WBNB, BTCB]
        .into_iter()
        .map(|t| (t, 18))
        .collect();
    let legs = graph_legs::legs_for(q, three, Side::Buy, 18, &dec).expect("ноги допущены");
    route_calc::eval(&q.ctx, &legs, Side::Buy, Decimal::ONE, Decimal::ONE).expect("путь считается")
}

async fn slippage(p: &arb::venues::bsc_dex::parts::Parts) {
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("exec.dex_slippage_pct", v, Some(json!("1")), "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn live_three_hops_match_quote() {
    let p = route_common::parts().await;
    mids_checked(&p);
    let q = p.quoting();
    let quoted = three_hop(&q);
    let hops: Vec<_> = quoted.hops.iter().map(|h| h.route_hop()).collect();
    let t = zero();
    let v = Via {
        q: &q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let live = live_hops::path(&v, &hops, Decimal::ONE)
        .await
        .expect("живой путь 3 хопов");
    assert_eq!(live.hops.len(), 3);
    assert_eq!(
        live.usdt, quoted.usdt,
        "та же математика, то же состояние — та же цена"
    );
    assert_eq!(live.coins, quoted.coins);
}

#[tokio::test]
async fn three_hop_steps_chain_on_router() {
    let p = route_common::parts().await;
    slippage(&p).await;
    let q = p.quoting();
    let path = three_hop(&q);
    let req = TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        side: Side::Buy,
        qty: Decimal::ONE,
        worst_price: path.usdt * Decimal::TWO,
        deal_id: "t".into(),
        route: None,
    };
    let steps = chain::steps(&q.ctx, &req, &path, Decimal::ONE, (18, 18)).expect("шаги");
    assert_eq!(steps.len(), 3);
    assert!(steps[0].from_wallet && !steps[0].to_wallet);
    assert!(!steps[1].from_wallet && !steps[1].to_wallet);
    assert!(!steps[2].from_wallet && steps[2].to_wallet);
    assert_eq!(steps[1].order.amount_in, ur::CONTRACT_BALANCE);
    assert_eq!(steps[2].order.amount_in, ur::CONTRACT_BALANCE);
    // Цепочка токенов: USDT → WBNB → BTCB → CAKE, у каждого шага свой минимум.
    let chain_tokens: Vec<(Address, Address)> = steps
        .iter()
        .map(|s| (s.order.token_in, s.order.token_out))
        .collect();
    assert_eq!(chain_tokens, vec![(USDT, WBNB), (WBNB, BTCB), (BTCB, CAKE)]);
    assert!(steps.iter().all(|s| !s.order.min_out.is_zero()));
    // Вход V2/V3 (формат PancakeSwap Infinity UR, Dispatcher.sol): (recipient, amountIn, amountOutMin, path, payerIsUser).
    for s in &steps {
        let (cmd, input) = chain::command(&q.ctx, s, ur::Format::Pancake, WALLET).expect("команда");
        let want = if s.entry.pool.kind == PoolKind::V2 {
            ur::V2_SWAP_EXACT_IN
        } else {
            ur::V3_SWAP_EXACT_IN
        };
        assert_eq!(cmd, want);
        let to = Address::abi_decode(&input[..32]).unwrap();
        let amount = U256::abi_decode(&input[32..64]).unwrap();
        let payer_user = bool::abi_decode(&input[128..160]).unwrap();
        assert_eq!(
            to,
            if s.to_wallet {
                WALLET
            } else {
                ur::ADDRESS_THIS
            }
        );
        assert_eq!(amount, s.order.amount_in);
        assert_eq!(payer_user, s.from_wallet);
    }
}

#[tokio::test]
async fn v4_middle_step_settles_from_router() {
    // Пул записи CAKE-WBNB с ключом Uniswap V4 (как в tests/bsc_dex_discovery.rs) — средний шаг пути: из роутера в роутер.
    let p = route_common::parts().await;
    let q = p.quoting();
    let mut e = q
        .reg
        .get(&arb::core::types::PoolRef::Contract(
            route_common::CAKE_WBNB_PCS,
        ))
        .unwrap();
    e.pool.kind = PoolKind::Cl;
    e.key = Some(PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: e.pool.token0,
        currency1: e.pool.token1,
        fee: 2500,
        tick_spacing: alloy_primitives::aliases::I24::try_from(50).unwrap(),
        hooks: Address::ZERO,
    }));
    let o = Order {
        token_in: WBNB,
        token_out: CAKE,
        amount_in: ur::CONTRACT_BALANCE,
        min_out: U256::from(1000u64),
    };
    let (cmd, input) = swap_v4::command(&q.ctx, &e, ur::Format::Uniswap20, &o, (false, false))
        .expect("средний шаг V4 собирается");
    assert_eq!(cmd, ur::V4_SWAP);
    let (actions, params) = <(Bytes, Vec<Bytes>)>::abi_decode_params(&input).unwrap();
    // SETTLE (0x0b) из баланса роутера, обмен (0x06), TAKE (0x0e) роутеру — Actions.sol v4-periphery / infinity-periphery.
    assert_eq!(actions.as_ref(), &[0x0b, 0x06, 0x0e]);
    let (c, amount, payer_user) = <(Address, U256, bool)>::abi_decode_params(&params[0]).unwrap();
    assert_eq!((c, amount, payer_user), (WBNB, ur::CONTRACT_BALANCE, false));
    let (c, to, amount) = <(Address, Address, U256)>::abi_decode_params(&params[2]).unwrap();
    assert_eq!((c, to, amount), (CAKE, ur::ADDRESS_THIS, U256::ZERO));
}

#[tokio::test]
async fn three_hop_leg_executable_within_max_hops() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let path = three_hop(&q);
    // В записи все DEX за одним условным Universal Router (route_common::dex_row) — нога одного роутера; бандл-канал.
    route_common::channel(&p, "club48", "bundle").await;
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("dex.bundle_max_routers", v.clone(), Some(json!(2)), "тест")
        .await
        .unwrap();
    graph_exec::executable(&q.ctx, std::slice::from_ref(&path), false)
        .expect("нога 3 хопов исполнима");
    // Решение 39 по налогу пула (решение 146): в пути через промежуточную шаг V3 с налогом стороны > 0 — сигнал.
    let mut taxed = path.clone();
    let h = taxed
        .hops
        .iter_mut()
        .find(|h| h.entry.pool.kind != PoolKind::V2)
        .expect("в пути записи есть шаг V3");
    h.usable.tax_pct = Decimal::ONE;
    let why = graph_exec::executable(&q.ctx, std::slice::from_ref(&taxed), false)
        .expect_err("налог и V3 через промежуточную");
    assert!(why.contains("решение 39"), "{why}");
    // dex.route_max_hops = 2 — та же нога уже длиннее предела.
    p.ctx
        .settings
        .set("dex.route_max_hops", v, Some(json!(2)), "тест")
        .await
        .unwrap();
    let why = graph_exec::executable(&q.ctx, &[path], false).expect_err("длиннее предела");
    assert!(why.contains("dex.route_max_hops"), "{why}");
}

/// Решение 39 в котировке графа: монета с налогом покупки (1%) — доля через промежуточную с пулом V3 не делает маршрут
/// исполнимым (причина в exec_note); одиночные пулы (одна доля, один хоп) налогом не ограничены.
#[tokio::test]
async fn taxed_coin_multihop_v3_not_executable_in_quote() {
    use arb::venues::bsc_dex::quote::graph_route;
    use arb::venues::bsc_dex::quote::steps::Names;
    let p = route_common::parts().await;
    // Решение 146: налог покупки 1 % — у пулов монеты (налог пула шага), как раньше у монеты целиком.
    for a in [
        route_common::CAKE_USDT_PCS,
        route_common::CAKE_USDT_UNI,
        route_common::CAKE_WBNB_PCS,
        route_common::CAKE_BTCB_UNI,
    ] {
        p.checks.update_pool(&PoolRef::Contract(a), |f| {
            f.taxes = Some(arb::venues::bsc_dex::check::state::PoolTaxes {
                buy_pct: Decimal::ONE,
                sell_pct: Some(Decimal::ZERO),
                transfer_pct: Decimal::ZERO,
            })
        });
    }
    let q = p.quoting();
    let t = TokenTaxes {
        buy_pct: Decimal::ONE,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let v = Via {
        q: &q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    // 50 000 CAKE на записи раскладываются в том числе по пути через WBNB (пул CAKE-WBNB — V3).
    let quote = graph_route::quote(
        &v,
        &names,
        &asset,
        Decimal::from(50_000u32),
        U256::from(1_000_000_000u64),
    )
    .await
    .expect("котировка графа");
    let r = quote.route.expect("маршрут");
    assert!(
        r.legs.iter().any(|l| l.hops.len() > 1),
        "есть доля через промежуточную"
    );
    assert!(!r.executable, "налог + V3 через промежуточную — неисполним");
    assert!(r.exec_note.unwrap_or_default().contains("решение 39"));
}

/// Ревью 122, пункт 3: налог на входе через исполнитель. Первый хоп пути записи с семейством вида шага V3 (slipstream,
/// не своя форма UR) — сигнал; вида SOLIDLY (прямой вход в пул) — исполнимо; две доли — сигнал (налог дважды).
#[tokio::test]
async fn taxed_input_via_executor_only_single_push_first_step() {
    let p = route_common::parts().await;
    let q = p.quoting();
    route_common::executor(&p, "0x00000000000000000000000000000000000000ee").await;
    let base = three_hop(&q);
    // Решение 146: налог пула первого шага 1 %; `sell` — монета пула на входе шага (продажа, налог на входе).
    let one = |kind: &str, sell: bool| {
        let mut path = base.clone();
        path.hops.truncate(1);
        let h = &mut path.hops[0];
        h.entry.family = families::make("x-review", kind).unwrap();
        h.usable.tax_pct = Decimal::ONE;
        if sell {
            h.token_in = h.entry.coin;
        }
        path
    };
    let v3 = one("slipstream", true);
    let why = graph_exec::executable(&q.ctx, std::slice::from_ref(&v3), false)
        .expect_err("налог на входе и первый шаг V3 через исполнитель");
    assert!(why.contains("налог на входе"), "{why}");
    // Без налога на входе (покупка) тот же путь исполним через исполнитель.
    graph_exec::executable(
        &q.ctx,
        std::slice::from_ref(&one("slipstream", false)),
        false,
    )
    .expect("покупка налоговой монеты: вход USDT");
    let sol = one("solidly_v1", true);
    graph_exec::executable(&q.ctx, std::slice::from_ref(&sol), false)
        .expect("первый шаг SOLIDLY на весь вход — прямой вход в пул, налог один раз");
    let why = graph_executor::tax(&[sol.clone(), sol]).expect_err("две доли");
    assert!(why.contains("долей"), "{why}");
}
