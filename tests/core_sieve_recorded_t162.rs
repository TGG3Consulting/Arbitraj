//! Файл: tests/core_sieve_recorded_t162.rs
//! Что проверяет: решения 372–374, ревью t162 Н5 — 18 записанных отказов кандидатов
//! (tests/data/cand_rejects_2026-11-16/cases.json, Calc на объёме записи) через НАСТОЯЩУЮ сборку границы assemble
//! (core/bound_in.rs): контекст направления (DirCtx) из чисел записи, котировки сторон записи как котировки снимка
//! (ставка комиссии — fee_of, как у живого кода), κ — из k·σ·√T контекста (assemble), пороги — из текста отказа.
//! Что из записи и что прокси (честно):
//! - σ и T — поля sigma и transit_s записи; k в записи нет — k восстанавливается из запаса записи R обращением формулы
//!   базовой части (решение 390, t176): R = выручка·(1 − e^(−k·σ·√T)) ⇒ k·σ·√T = −ln(1 − R / выручка). Записи сделаны
//!   до решения 390 (запас R = k·σ·√T·выручка), поэтому восстановленное k — не k бота, но κ границы
//!   e^(−k·σ·√T) − r_B = 1 − R/выручка − r_B — ровно та доля, что была в записи (как до 390). Если в момент
//!   записи действовала доля закрытия (решение 240, profit.closure_share), запас записи больше базовой части и k здесь
//!   больше живого — эта граница ниже живой (живой код отсечёт не больше, чем здесь);
//! - стаканов и состояний пулов в записи нет (README записи): стакан биржи — один уровень по средней цене записи
//!   (покупка — глубиной size.max_qty, продажа — 10× size.max_qty), верх DEX ρ — средняя цена записи (продажа — sell_avg,
//!   покупка — 1/buy_avg). Настоящий ρ живого кода — верх по пулам, не меньше средней, а лучший уровень стакана не хуже
//!   средней ⇒ живой код отсекает НЕ БОЛЬШЕ, чем здесь; это иллюстрация, не прогноз;
//! - фикс вывода A — строка WithdrawFee записи (могла быть уже с поправкой по фактам — в записи не различить), доли нет;
//!   налог перевода — TransferTax / (отправлено − фикс); возврат USDT с биржи B — строка UsdtReturn записи как фикс
//!   (доли в записи нет; если она была, живой фикс меньше — живой код отсекает не больше).
//!
//! Итог на записи (то, что проверяет тест): граница на объёме записи ≥ прибыли записи у всех 18; отсечено 17 из 18,
//! не отсечён NXT (почти-сделка: прибыль −0,0041 при пороге 0,005). Сколько из 18 отсечёт живой код на настоящих
//! стаканах и пулах — из записи не выводится (не больше 17).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{bound_in, dirctx, registry}, tests/data/cand_rejects_2026-11-16.

use std::str::FromStr;

use arb::core::bound_in::{SideIn, assemble};
use arb::core::dirctx::DirCtx;
use arb::core::registry::Presence;
use arb::core::types::{
    Asset, CoinNetworkInfo, Cost, CostKind, Direction, OrderBook, PriceLevel, Quote, Side, VenueId,
};
use arb::gateway::rate_bound::DexBound;
use chrono::Utc;
use rust_decimal::{Decimal, MathematicalOps};
use serde_json::Value;

/// Десятичное поля записи.
fn dv(v: &Value) -> Decimal {
    Decimal::from_str(v.as_str().unwrap()).unwrap()
}

/// Сеть площадки с фиксом вывода (доли нет — в записи её нет).
fn net(venue: &str, ticker: &str, fix: Decimal) -> CoinNetworkInfo {
    CoinNetworkInfo {
        venue: VenueId(venue.into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: None,
        },
        network: "BEP20".into(),
        deposit_enabled: true,
        withdraw_enabled: true,
        withdraw_fee: Some(fix),
        withdraw_fee_pct: None,
        withdraw_min: None,
        withdraw_max: None,
        deposit_min: None,
        deposit_confirmations: None,
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    }
}

/// Присутствие стороны: биржа — с сетью, DEX — без.
fn presence(venue: &str, ticker: &str, info: Option<CoinNetworkInfo>) -> Presence {
    Presence {
        venue: VenueId(venue.into()),
        ticker: ticker.into(),
        network: Some("BEP20".into()),
        can_receive: true,
        can_send: true,
        can_buy: true,
        can_sell: true,
        excluded: vec![],
        info,
        via: None,
    }
}

/// Котировка стороны записи (для ставки комиссии — только торговые комиссии не в цене).
fn quote(q: &Value, asset: &Asset, side: Side) -> Quote {
    let costs = q["costs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "TradeFee")
        .map(|c| {
            Cost::new(
                CostKind::TradeFee,
                dv(&c["amount"]),
                c["asset"].as_str().unwrap(),
                c["in_price"].as_bool().unwrap(),
            )
        })
        .collect();
    Quote {
        venue: VenueId(q["venue"].as_str().unwrap().into()),
        asset: asset.clone(),
        side,
        qty: dv(&q["qty"]),
        fillable_qty: dv(&q["fillable_qty"]),
        avg_price: dv(&q["avg_price"]),
        costs,
        route: None,
        signal_route: None,
        pick: None,
        at: Utc::now(),
    }
}

/// Стакан из одного уровня.
fn book(venue: &str, asset: &Asset, price: Decimal, qty: Decimal) -> OrderBook {
    let l = vec![PriceLevel { price, qty }];
    OrderBook {
        venue: VenueId(venue.into()),
        asset: asset.clone(),
        bids: l.clone(),
        asks: l,
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Порог из текста отказа: (число, это %).
fn threshold(msg: &str) -> (Decimal, bool) {
    let tail = msg.split("ниже порога ").nth(1).unwrap();
    let t: String = tail
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.' || *ch == '-')
        .collect();
    let pct = tail[t.len()..].starts_with('%');
    (Decimal::from_str(&t).unwrap(), pct)
}

/// Одна запись: (тикер, отсечена). Паника — граница на объёме записи ниже прибыли записи.
fn one(c: &Value) -> (String, bool) {
    let ticker = c["направление"]["asset"]["ticker"]
        .as_str()
        .unwrap()
        .to_string();
    let (buy_on, sell_on) = (
        c["направление"]["buy_on"].as_str().unwrap(),
        c["направление"]["sell_on"].as_str().unwrap(),
    );
    let (dex_a, dex_b) = (buy_on == "bsc_dex", sell_on == "bsc_dex");
    let asset = Asset {
        ticker: ticker.clone(),
        bsc_contract: None,
    };
    let (q, buy, sell) = (dv(&c["qty"]), dv(&c["buy_avg"]), dv(&c["sell_avg"]));
    let (pro, profit) = (dv(&c["proceeds"]), dv(&c["profit_usd"]));
    let (sigma, transit) = (dv(&c["sigma"]), dv(&c["transit_s"]));
    let qmax = dv(&c["size"]["max_qty"]);
    let lines = c["строки_затрат"].as_array().unwrap();
    let sum = |kind: &str| -> Decimal {
        lines
            .iter()
            .filter(|l| l["вид"] == kind)
            .map(|l| dv(&l["сумма"]))
            .sum()
    };
    let (buy_q, sell_q) = (
        quote(&c["покупка"], &asset, Side::Buy),
        quote(&c["продажа"], &asset, Side::Sell),
    );
    let fee_coin: Decimal = buy_q
        .costs
        .iter()
        .filter(|x| x.asset == ticker)
        .map(|x| x.amount)
        .sum();
    let w = sum("WithdrawFee");
    let sent = q - fee_coin;
    let tax = if sent > w {
        sum("TransferTax") / (sent - w) * Decimal::ONE_HUNDRED
    } else {
        Decimal::ZERO
    };
    let sqrt_t = transit.sqrt().unwrap();
    let k = -(Decimal::ONE - sum("PriceMoveReserve") / pro).ln() / (sigma * sqrt_t);
    let ret = sum("UsdtReturn");
    let cx = DirCtx {
        dir: Direction {
            asset: asset.clone(),
            buy_on: VenueId(buy_on.into()),
            sell_on: VenueId(sell_on.into()),
        },
        a: presence(buy_on, &ticker, (!dex_a).then(|| net(buy_on, &ticker, w))),
        b: presence(
            sell_on,
            &ticker,
            (!dex_b).then(|| net(sell_on, &ticker, Decimal::ZERO)),
        ),
        net_a: "BEP20".into(),
        net_b: "BEP20".into(),
        quote: vec!["USDT".into(), "USDT".into()],
        usdt: Asset {
            ticker: "USDT".into(),
            bsc_contract: None,
        },
        cex: vec![],
        sigma,
        transit_s: transit,
        k,
        h: Decimal::ONE,
        closure_share: None,
        keep: arb::core::reserve::keep_of(k, sigma, transit),
        sigma_check: None,
        return_per_trade: !dex_b,
        pair_a: None,
        pair_b: None,
        usdt_b: (!dex_b).then(|| net(sell_on, "USDT", ret)),
        transfer_tax: (!dex_a).then_some(tax),
        sell_max: None,
        band_a: None,
        band_b: None,
    };
    let (book_a, book_b) = (
        book(buy_on, &asset, buy, qmax),
        book(sell_on, &asset, sell, qmax * Decimal::TEN),
    );
    let (rho_a, rho_b) = (
        DexBound {
            rho: Decimal::ONE / buy,
            pools: 0,
            hops: 0,
        },
        DexBound {
            rho: sell,
            pools: 0,
            hops: 0,
        },
    );
    let a = if dex_a {
        SideIn::Dex(&rho_a)
    } else {
        SideIn::Book {
            book: &book_a,
            sample: &buy_q,
            ticker: &ticker,
        }
    };
    let b = if dex_b {
        SideIn::Dex(&rho_b)
    } else {
        SideIn::Book {
            book: &book_b,
            sample: &sell_q,
            ticker: &ticker,
        }
    };
    let bound = assemble(&cx, &a, &b, None).unwrap_or_else(|e| panic!("{ticker}: {e}"));
    let (u, _) = bound.u_at(q).unwrap();
    assert!(
        u >= profit,
        "{ticker}: граница {u} на объёме записи < прибыли записи {profit}"
    );
    let (t, pct) = threshold(c["сообщение"].as_str().unwrap());
    let best = bound
        .best(Some(if pct { t } else { -Decimal::ONE }))
        .unwrap();
    let cut = if pct {
        best.below_pct()
    } else {
        best.below_usd(t)
    };
    println!(
        "{ticker}: прибыль {profit}, U(q) {u}, max U {}, κ {} → {}",
        best.max_u,
        bound.kappa,
        if cut {
            "отсечён"
        } else {
            "в расчёт"
        }
    );
    (ticker, cut)
}

/// 18 записанных отказов через assemble: граница ≥ прибыли записи; отсечено 17, NXT — нет (см. шапку).
#[test]
fn recorded_rejects_17_of_18_via_assemble() {
    let all: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/cand_rejects_2026-11-16/cases.json").unwrap(),
    )
    .unwrap();
    let mut res = Vec::new();
    for (_, group) in all.as_object().unwrap() {
        for c in group.as_array().unwrap() {
            res.push(one(c));
        }
    }
    let kept: Vec<String> = res.iter().filter(|r| !r.1).map(|r| r.0.clone()).collect();
    assert_eq!(res.len(), 18);
    assert_eq!(kept, vec!["NXT".to_string()], "в расчёт: {kept:?}");
}
