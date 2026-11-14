//! Файл: tests/bsc_dex_mid_dex.rs
//! Что делает: решение 276 — промежуточная монета, адрес которой не знает биржа (dex.intermediate_dex_priced), на
//! записи с цепи tests/data/fstswap_fist_2026-11-14/calls.json (eth_call, один блок 0x7f680d1): symbol()/decimals()
//! FIST и чужой обёртки fstswap, резервы пар fstswap FIST/USDT и ETH/FIST (фабрика 0x9A27…dce, getPair/token0/
//! token1/factory). Проверки: (1) pick — подпись symbol() с цепи; совпадение с «BNB», тикером биржи (без учёта
//! регистра), пустой и не-ABI ответ (32-байтное слово — ответ decimals(), такой же формы, как bytes32 у старых
//! токенов) — подпись адресом; «WBNB» чужой обёртки — адресом, если занят; (2) путь USDT → FIST → ETH через
//! Via::calc на записанных резервах: цена ETH рядом с серединой рынка из тех же резервов; Names::of даёт подпись;
//! затраты в FIST — только «в цене» (in_price = true); (3) mids::read: флаг не задан — прежний отказ текст в текст,
//! флаг = да — монета в списке с подписью (узла в тесте нет — подпись адресом) и запись журнала.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote, node, parts}, arb::settings (SQLite во временной папке).
//! Комиссия пула V2 0,25 % — значение теста (настройка dex.v2_fee_pct), не факт fstswap; математика — V2 (пары
//! fstswap отвечают getReserves/token0/token1/factory интерфейса V2 — факт записи).

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::types::{CostKind, Side, TokenTaxes};
use arb::venues::bsc_dex::node::abi::IERC20;
use arb::venues::bsc_dex::node::call::decode;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mid_dex::pick;
use arb::venues::bsc_dex::pools::mids::{self, Mid};
use arb::venues::bsc_dex::quote::steps::{self, Names};
use arb::venues::bsc_dex::quote::via::Via;

mod mid_dex_common;
use mid_dex_common::{clean, ctx, pair, prepare};

fn rec() -> Value {
    serde_json::from_str(include_str!("data/fstswap_fist_2026-11-14/calls.json")).unwrap()
}
fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
}
fn addr(v: &Value, k: &str) -> Address {
    v["tokens"][k].as_str().unwrap().parse().unwrap()
}
fn symbol(v: &Value, k: &str) -> Option<String> {
    decode::<IERC20::symbolCall>(&hex(v[k].as_str().unwrap())).ok()
}
/// Резервы (r0, r1) из ответа getReserves() записи.
fn reserves(v: &Value, pair: &str) -> (u128, u128) {
    let raw = hex(v["pairs"][pair]["getReserves()_0x0902f1ac"]
        .as_str()
        .unwrap());
    let w = |i: usize| U256::from_be_slice(&raw[i * 32..i * 32 + 32]).to::<u128>();
    (w(0), w(1))
}

#[test]
fn pick_collisions() {
    let v = rec();
    let fist = addr(&v, "FIST");
    let wrapper = addr(&v, "WRAPPER");
    let busy = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let s = symbol(&v, "FIST.symbol()_0x95d89b41");
    assert_eq!(s.as_deref(), Some("FIST"));
    assert_eq!(
        pick(s.clone(), fist, &busy(&["USDT", "BNB", "ETH"])),
        "FIST"
    );
    let as_addr = format!("{fist:#x}");
    assert_eq!(as_addr, "0xc9882def23bc42d53895b8361d0b1edc7570bc6a");
    // Совпало с тикером биржи (без учёта регистра) — адрес.
    assert_eq!(pick(s, fist, &busy(&["fist"])), as_addr);
    // «BNB» — тикер газа: ни одна промежуточная его не получает.
    assert_eq!(pick(Some(" BNB ".into()), fist, &busy(&["BNB"])), as_addr);
    // Пусто и не-ABI ответ (32-байтное слово decimals() — не строка ABI) — адрес.
    assert_eq!(pick(Some("  ".into()), fist, &[]), as_addr);
    let word = symbol(&v, "FIST.decimals()_0x313ce567");
    assert_eq!(word, None);
    assert_eq!(pick(word, fist, &[]), as_addr);
    // Чужая обёртка fstswap: symbol() «WBNB»; занят — адрес, свободен — «WBNB» (но не «BNB»: цены 1:1 нет).
    let w = symbol(&v, "WRAPPER.symbol()_0x95d89b41");
    assert_eq!(w.as_deref(), Some("WBNB"));
    assert_eq!(
        pick(w.clone(), wrapper, &busy(&["WBNB", "BNB"])),
        format!("{wrapper:#x}")
    );
    assert_eq!(pick(w, wrapper, &busy(&["BNB"])), "WBNB");
}

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
    let label = pick(
        symbol(&v, "FIST.symbol()_0x95d89b41"),
        fist,
        &["USDT".into(), "ETH".into(), "BNB".into()],
    );
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

#[tokio::test]
async fn read_flag_off_and_on() {
    let v = rec();
    let fist = addr(&v, "FIST");
    let p = Parts::new(ctx("mid-dex-read", false).await);
    let c = &p.ctx;
    let s = arb::settings::scope::Scope::Venue("bsc_dex".into());
    c.settings
        .set(
            "dex.intermediate_enabled",
            s.clone(),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
    c.settings
        .set(
            "dex.intermediate_tokens",
            s.clone(),
            Some(json!([format!("{fist:#x}")])),
            "тест",
        )
        .await
        .unwrap();
    // Флаг не задан — прежний отказ, текст в текст.
    assert!(mids::read(c, &p.tokens).await.unwrap().is_empty());
    let old = format!(
        "Промежуточная монета {fist}: биржа не знает этот адрес и это не обёртка нативной монеты (WETH() роутера V2, dex.native_wrapped) — затраты не пересчитать, не используется"
    );
    let log = c.journal.list(50, None).await.unwrap();
    assert!(log.iter().any(|e| e.message == old), "{log:?}");
    // Флаг = да — монета используется; узла нет — symbol() не прочитан, подпись адресом.
    c.settings
        .set("dex.intermediate_dex_priced", s, Some(json!(true)), "тест")
        .await
        .unwrap();
    let got = mids::read(c, &p.tokens).await.unwrap();
    assert_eq!(
        got,
        vec![Mid {
            address: fist,
            ticker: format!("{fist:#x}")
        }]
    );
    let log = c.journal.list(50, None).await.unwrap();
    let want = format!(
        "Промежуточная {fist}: биржа не знает адрес — подпись {fist:#x} с цепи, цена — пулом с USDT (dex.intermediate_dex_priced)"
    );
    assert!(log.iter().any(|e| e.message == want), "{log:?}");
}
