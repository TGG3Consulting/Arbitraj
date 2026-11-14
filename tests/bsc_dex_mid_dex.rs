//! Файл: tests/bsc_dex_mid_dex.rs
//! Что делает: решение 276 — промежуточная монета, адрес которой не знает биржа (dex.intermediate_dex_priced), на
//! записи с цепи tests/data/fstswap_fist_2026-11-14/calls.json (eth_call, один блок 0x7f680d1): резервы пар fstswap
//! FIST/USDT и ETH/FIST (фабрика 0x9A27…dce, getPair/token0/token1/factory), symbol()/decimals() FIST. Проверка: путь
//! USDT → FIST → ETH через Via::calc на записанных резервах: цена ETH рядом с серединой рынка из тех же резервов;
//! Names::of даёт подпись с цепи; затраты в FIST — только «в цене» (in_price = true).
//! Подпись (pick) — tests/bsc_dex_mid_dex_pick.rs; mids::read и занятые тикеры — tests/bsc_dex_mid_dex_read.rs;
//! расчёт ядра с затратой в FIST — tests/core_mid_dex_cost.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote, parts}, tests/mid_dex_common.
//! Комиссия пула V2 0,3 % (настройка dex.v2_fee_pct) — факт fstswap: tests/bsc_dex_fstswap_fee.rs; математика — V2
//! (пары fstswap отвечают getReserves/token0/token1/factory интерфейса V2 — факт записи).

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use arb::core::types::{CostKind, Side, TokenTaxes};
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mid_dex::pick;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::quote::steps::{self, Names};
use arb::venues::bsc_dex::quote::via::Via;

mod mid_dex_common;
use mid_dex_common::rec::{addr, hex, rec, reserves, symbol};
use mid_dex_common::{clean, ctx, pair, prepare};

#[tokio::test]
async fn usdt_fist_eth_path() {
    let v = rec();
    let (usdt, fist, eth) = (addr(&v, "USDT"), addr(&v, "FIST"), addr(&v, "ETH"));
    let dec = |k: &str| U256::from_be_slice(&hex(v[k].as_str().unwrap())).to::<u8>();
    let (fd, ed, ud) = (
        dec("FIST.decimals()_0x313ce567"),
        dec("ETH.decimals()_0x313ce567"),
        dec("USDT.decimals()_0x313ce567"),
    );
    assert_eq!((fd, ed, ud), (6, 18, 18));
    let p = Parts::new(ctx("mid-dex-path", true).await);
    let q = p.quoting();
    // token0/token1 из записи: FIST/USDT — token0 = USDT; ETH/FIST — token0 = ETH.
    let (fu_usdt, fu_fist) = reserves(&v, "FIST_USDT");
    let (ef_eth, ef_fist) = reserves(&v, "ETH_FIST");
    let pa = |k: &str| {
        v["pairs"][k]["pair"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    };
    // Предел подписи 16 — значение теста (dex.intermediate_label_max_len).
    let (label, why) = pick(
        symbol(&v, "FIST.symbol()_0x95d89b41"),
        fist,
        &["USDT".into(), "ETH".into(), "BNB".into()],
        Some(16),
    );
    assert_eq!(why, None);
    let p1 = pair(pa("FIST_USDT"), fist, usdt, &label, fu_fist, fu_usdt);
    let p2 = pair(pa("ETH_FIST"), eth, fist, "ETH", ef_eth, ef_fist);
    prepare(&p, &[&p1, &p2], usdt, eth);
    for (a, d) in [(fist, fd), (eth, ed), (usdt, ud)] {
        p.tokens.remember_decimals(a, d);
    }
    p.checks.set_coin(clean(&label, fist));
    p.checks.set_coin(clean("ETH", eth));
    let mid = Mid {
        address: fist,
        ticker: label.clone(),
    };
    p.mids.set(vec![mid.clone()]);
    let names = Names {
        usdt: (usdt, "USDT"),
        coin: (eth, "ETH"),
        mids: &p.mids,
    };
    assert_eq!(names.of(&q.ctx, fist).unwrap(), "FIST");
    // Середина рынка из тех же резервов: USDT за FIST × FIST за ETH.
    let d = |x: u128, k: u32| Decimal::from(x) / Decimal::from(10u128.pow(k));
    let mid_px = d(fu_usdt, 18) / d(fu_fist, 6) * (d(ef_fist, 6) / d(ef_eth, 18));
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let qty = Decimal::from_str("0.01").unwrap();
    let band = Decimal::from_str("1.01").unwrap();
    for side in [Side::Buy, Side::Sell] {
        let via = Via {
            q: &q,
            coin: eth,
            usdt,
            side,
            taxes: &t,
            coin_dec: ed,
            usdt_dec: ud,
        };
        let (a, b) = via.pools(&mid, &|_| true).expect("пары допущены");
        let path = via
            .calc(&mid, fd, &a[0], &b[0], qty)
            .expect("путь считается");
        let price = path.usdt / path.coins;
        let hops: Vec<_> = path
            .hops
            .iter()
            .map(|h| (h.token_in, h.token_out))
            .collect();
        match side {
            Side::Buy => {
                assert_eq!(hops, vec![(usdt, fist), (fist, eth)]);
                assert!(price > mid_px && price < mid_px * band, "{price} {mid_px}");
            }
            Side::Sell => {
                assert_eq!(hops, vec![(eth, fist), (fist, usdt)]);
                assert!(price < mid_px && price > mid_px / band, "{price} {mid_px}");
            }
        }
        let costs = steps::path_costs(
            &q.ctx,
            &path,
            side,
            &t,
            U256::from(1_000_000_000u64),
            &names,
        )
        .unwrap();
        let in_fist: Vec<_> = costs.iter().filter(|c| c.asset == "FIST").collect();
        assert!(in_fist.iter().any(|c| c.kind == CostKind::PoolFee));
        assert!(
            in_fist.iter().all(|c| c.in_price),
            "затраты в FIST — только «в цене»"
        );
        let s2 = costs
            .iter()
            .filter_map(|c| c.step.clone())
            .find(|s| s.index == 2)
            .unwrap();
        assert!(s2.from == "FIST" || s2.to == "FIST");
    }
}
