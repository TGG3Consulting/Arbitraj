//! Файл: tests/bsc_dex_best_of.rs
//! Что делает: проверяет чистый выбор «лучший из двух» (решение 123, venues/bsc_dex/best_of/choose.rs) на настоящих
//! котировках записи маршрута CAKE: routes_2026-10-29 (блок 130521896) и routes_2026-10-30 (блок 130556914) — те же
//! шесть пулов на двух блоках дают две реальные котировки одного объёма с разной ценой. Записей ответов KyberSwap
//! /routes в tests/ нет (вопрос Тиграну — записать на сервере); выбор не знает, откуда котировка (KyberSwap или
//! своя), поэтому в роли «котировки KyberSwap» — записанная котировка другого блока (route: None, как у KyberSwap).
//! Ошибки KyberSwap — настоящие коды шлюза (RateLimited — HTTP 429, Timeout, NoRoute). Моков площадок нет.
//! Проверяется: ошибка одной стороны → другая; ошибки обеих → отказ с причинами обеих; своя неисполнимая
//! (кросс-DEX сплит графа, решение 84) не берётся; покупка — меньше USDT, продажа — больше; полный объём лучше
//! неполного; худшая цена ядра на сделке отсекает сторону; отказы «до отправки» для отката на KyberSwap.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::best_of::{choose, extra}, arb::venues::bsc_dex::quote, tests/route_common.

mod route_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::{Asset, Quote, Side, TokenTaxes};
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::best_of::choose::{self, Offer, Who};
use arb::venues::bsc_dex::best_of::extra;
use arb::venues::bsc_dex::best_of::trade::pre_send;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, direct_route, graph_route};

use route_common::{CAKE, USDT};

const GAS: u64 = 1_000_000_000;
const DAY2: &str = "tests/data/routes_2026-10-30";

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

/// Котировка классики (прямой путь, исполнима) по записи окружения. Ошибок нет (тест падает).
async fn classic(q: &Quoting, side: Side, qty: u32) -> Quote {
    let t = zero();
    let v = Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    let entries: Vec<Entry> = q
        .reg
        .by_pair(CAKE, USDT)
        .into_iter()
        .filter(|e| e.ticker == "CAKE")
        .collect();
    let f = |_: &Entry| true;
    direct_route::quote(
        &v,
        &names,
        &cake(),
        Decimal::from(qty),
        entries,
        &[],
        &f,
        U256::from(GAS),
    )
    .await
    .expect("классика считает")
}

/// Две реальные котировки одного объёма на двух блоках: (блок 29.10 — «своя», блок 30.10 — в роли KyberSwap,
/// без маршрута DEX, как отдаёт KyberSwap).
async fn pair(side: Side, qty: u32) -> (Quote, Quote) {
    let own = classic(&route_common::parts().await.quoting(), side, qty).await;
    let mut kyber = classic(&route_common::parts_from(DAY2).await.quoting(), side, qty).await;
    kyber.route = None;
    assert_ne!(
        own.avg_price, kyber.avg_price,
        "записи двух блоков дают разную цену"
    );
    (own, kyber)
}

fn offer(q: &Quote) -> Result<Offer<'_>, String> {
    Ok(Offer {
        quote: q,
        extra_usdt: None,
    })
}

/// Итог стороны за одно количество без затрат вне цены — то, что сравнивает choose (extra_usdt = None у обеих).
fn total(q: &Quote) -> Decimal {
    q.avg_price * q.fillable_qty
}

#[tokio::test]
async fn kyber_error_takes_own() {
    // Лимит KyberSwap (HTTP 429 → RateLimited) больше не срывает сделку: своя годна — своя.
    let (own, _) = pair(Side::Buy, 1000).await;
    for code in [
        ErrorCode::RateLimited,
        ErrorCode::Timeout,
        ErrorCode::NoRoute,
    ] {
        let k = Err(format!("{code:?}: KyberSwap не ответил"));
        let c = choose::choose(Side::Buy, None, k, offer(&own)).expect("своя годна");
        assert_eq!(c.who, Who::Own);
        assert!(c.why.contains("KyberSwap не годится") && c.gain_usdt.is_none());
    }
}

#[tokio::test]
async fn own_error_takes_kyber_and_both_fail() {
    let (_, kyber) = pair(Side::Sell, 1000).await;
    let c = choose::choose(Side::Sell, None, offer(&kyber), Err("StaleData".into())).unwrap();
    assert_eq!(c.who, Who::Kyber);
    let e = choose::choose(
        Side::Sell,
        None,
        Err("RateLimited".into()),
        Err("NoRoute".into()),
    )
    .expect_err("обе не годятся — отказ");
    assert!(e.contains("KyberSwap: RateLimited") && e.contains("свой: NoRoute"));
}

#[tokio::test]
async fn own_not_executable_is_not_taken() {
    // Кросс-DEX сплит графа (решение 84) неисполним как есть — своя не годится, хоть и найдена.
    let p = route_common::parts().await;
    let q = p.quoting();
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
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
    };
    let qty = Decimal::from(50_000u32);
    let split = graph_route::quote(&v, &names, &cake(), qty, U256::from(GAS))
        .await
        .unwrap();
    assert!(!split.route.as_ref().unwrap().executable);
    let (_, kyber) = pair(Side::Buy, 50_000).await;
    let c = choose::choose(Side::Buy, None, offer(&kyber), offer(&split)).unwrap();
    assert_eq!(c.who, Who::Kyber);
    assert!(c.why.contains("не исполним как есть"), "{}", c.why);
    // Без маршрута в котировке своя тоже не годится.
    assert!(choose::usable_own(&kyber).is_err());
}

#[tokio::test]
async fn buy_cheaper_and_sell_higher_win() {
    for side in [Side::Buy, Side::Sell] {
        let (own, kyber) = pair(side, 1000).await;
        let c = choose::choose(side, None, offer(&kyber), offer(&own)).unwrap();
        let (to, tk) = (total(&own), total(&kyber));
        let own_better = if side == Side::Buy { to < tk } else { to > tk };
        assert_eq!(
            c.who,
            if own_better { Who::Own } else { Who::Kyber },
            "{side:?}: {}",
            c.why
        );
        assert_eq!(c.gain_usdt, Some((to - tk).abs()));
        assert!(
            c.why.contains("KyberSwap") && c.why.contains("vs свой") && c.why.contains("без газа")
        );
    }
}

#[tokio::test]
async fn full_volume_beats_partial() {
    // Неполный объём: та же записанная котировка, но запрошено больше, чем исполнимо (как живой случай RVV, решение
    // 111: 359317.97 из 360732.6) — ядро ужмёт объём; полная сторона берётся даже при худшей цене.
    let (own, kyber) = pair(Side::Buy, 1000).await;
    let mut kyber_part = kyber.clone();
    kyber_part.qty += Decimal::ONE;
    let c = choose::choose(Side::Buy, None, offer(&kyber_part), offer(&own)).unwrap();
    assert_eq!(c.who, Who::Own, "{}", c.why);
    assert!(c.why.contains("KyberSwap исполняет только"), "{}", c.why);
    let mut own_part = own.clone();
    own_part.qty += Decimal::ONE;
    let c = choose::choose(Side::Buy, None, offer(&kyber), offer(&own_part)).unwrap();
    assert_eq!(c.who, Who::Kyber, "{}", c.why);
    assert!(c.why.contains("свой исполняет только"), "{}", c.why);
}

#[tokio::test]
async fn worst_price_cuts_side_on_trade() {
    // На сделке худшая цена ядра между двумя средними: более дорогая покупка не годится — берётся другая.
    let (own, kyber) = pair(Side::Buy, 1000).await;
    let worst = (own.avg_price + kyber.avg_price) / Decimal::TWO;
    let c = choose::choose(Side::Buy, Some(worst), offer(&kyber), offer(&own)).unwrap();
    let cheaper = if own.avg_price < kyber.avg_price {
        Who::Own
    } else {
        Who::Kyber
    };
    assert_eq!(c.who, cheaper);
    assert!(c.why.contains("хуже допустимой"), "{}", c.why);
    let low = own.avg_price.min(kyber.avg_price) / Decimal::TWO;
    assert!(choose::choose(Side::Buy, Some(low), offer(&kyber), offer(&own)).is_err());
}

#[tokio::test]
async fn gas_without_bnb_rate_compares_both_without_it() {
    // В записи нет пула с тикером BNB в реестре (все записи пулов — монеты CAKE) — газ в BNB не переводится в USDT:
    // extra::usdt = None, и сравнение идёт у обеих без затрат вне цены, с пометкой (честно, а не газ у одной).
    let p = route_common::parts().await;
    let q = p.quoting();
    let (own, kyber) = pair(Side::Sell, 1000).await;
    assert!(
        own.costs.iter().any(|c| c.asset == "BNB" && !c.in_price),
        "газ своей — в BNB"
    );
    let eo = extra::usdt(&q, &own).await;
    assert_eq!(eo, None, "курса BNB в записи нет");
    let o = Offer {
        quote: &own,
        extra_usdt: eo,
    };
    let k = Offer {
        quote: &kyber,
        extra_usdt: Some(Decimal::ZERO),
    };
    let c = choose::choose(Side::Sell, None, Ok(k), Ok(o)).unwrap();
    assert!(c.why.contains("без газа"), "{}", c.why);
}

#[tokio::test]
async fn extra_costs_can_flip_the_winner() {
    // Затраты вне цены (газ, approve, чаевые) в USDT входят в итог: победителю по цене дать затраты больше его
    // выигрыша — побеждает другой. Покупка: + к отданному; продажа: − от полученного.
    for side in [Side::Buy, Side::Sell] {
        let (own, kyber) = pair(side, 1000).await;
        let plain = choose::choose(side, None, offer(&kyber), offer(&own)).unwrap();
        let gain = plain.gain_usdt.unwrap();
        let heavy = Some(gain * Decimal::TWO);
        let (ek, eo) = match plain.who {
            Who::Own => (Some(Decimal::ZERO), heavy),
            Who::Kyber => (heavy, Some(Decimal::ZERO)),
        };
        let k = Ok(Offer {
            quote: &kyber,
            extra_usdt: ek,
        });
        let o = Ok(Offer {
            quote: &own,
            extra_usdt: eo,
        });
        let c = choose::choose(side, None, k, o).unwrap();
        assert_ne!(c.who, plain.who, "{side:?}: {}", c.why);
        assert!(c.why.contains("с газом"), "{}", c.why);
        assert_eq!(
            c.gain_usdt,
            Some(gain),
            "{side:?}: выигрыш нового победителя = 2g − g"
        );
    }
}

#[test]
fn pre_send_only_before_swap_left() {
    assert!(
        pre_send(ErrorCode::QuoteMismatch, false),
        "симуляция/цена — до отправки"
    );
    assert!(pre_send(ErrorCode::NoRoute, false));
    assert!(
        pre_send(ErrorCode::InvalidAddress, false),
        "проверка исполнителя"
    );
    assert!(
        !pre_send(ErrorCode::QuoteMismatch, true),
        "транзакция ушла — повторов нет"
    );
    assert!(!pre_send(ErrorCode::SwapReverted, true));
    assert!(!pre_send(ErrorCode::OutcomeUnknown, false));
    assert!(
        !pre_send(ErrorCode::ChannelRejected, false),
        "канал мог разослать — не повторяем"
    );
    assert!(!pre_send(ErrorCode::TxNotIncluded, false));
}
