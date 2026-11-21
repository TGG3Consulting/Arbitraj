//! Файл: tests/bsc_dex_squadswap_v3_live.rs
//! Что проверяет: решение 281 — SquadSwap V3 (squadswap-v3-bsc, вид v3_fork_pancake) на РЕАЛЬНЫХ ответах узла BSC,
//! записанных на одном блоке 133618816 (tests/data/squadswap_v3_2026-11-15/probe.json): пулы Cake/WBNB 0x9fb9…bd33,
//! LTC/WBNB 0xc144…8c53, USDT/WBNB 0xa95d…6dae.
//! 1. Реестр фабрики хранит пул под defaultFee(), не под fee(): с ответом метода dex.v3_registry_fee_method (седьмой
//!    ответ полей) getPool строится по defaultFee и реестр возвращает сам пул; без настройки — по fee(), реестр даёт 0,
//!    отказ прежним текстом «фабрика DEX не знает этот пул».
//! 2. fee() пула = getFee(pool, defaultFee) менеджера; poolManager() фабрики = менеджер.
//! 3. QuoterV2 с ключом defaultFee отвечает, с fee() — откатывается; registry_key::quoter_fee выбирает ключ.
//! 4. Математика бота (math/swap_cl.rs) с ТЕКУЩЕЙ fee() пула и тиками TickLens = QuoterV2 и виртуальному квотеру (новый
//!    артефакт со squadV3SwapCallback) до wei на 1/100/10000 $ в обе стороны; частичный расчёт (цена ушла за
//!    записанные слова) — не больше квотера.
//! 5. Свежая fee() в голове пачки (fee_fresh::split) отделяется, остаток разбирается прежним parse_head.
//! 6. Живой лог Swap — ABI PancakeSwap V3 (9 полей); фабрика Squad на swapFee(0x0, 0) откатывается (fee_probe не
//!    срабатывает ложно).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, pools::{abi_v2v3, families, fee_fresh, abi_squad, fee_probe,
//! load_batch, load_batch_v2v3, registry_key, ticks, verify}}.

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, LogData, U256, keccak256};
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::pools::abi_v2v3::{IPancakeV3Pool, IV3Factory};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::load_batch::Want;
use arb::venues::bsc_dex::pools::load_batch_v2v3::{self as v23, PoolStateTick, v3_ticks};
use arb::venues::bsc_dex::pools::ticks::TickMap;
use arb::venues::bsc_dex::pools::verify::{Read, fill, registered, registry_call_with};
use arb::venues::bsc_dex::pools::{abi_squad, fee_fresh, fee_probe, registry_key};

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/squadswap_v3_2026-11-15/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
}

fn bytes(v: &Value) -> Bytes {
    Bytes::from(hex(v.as_str().unwrap()))
}

/// Слово `i` ответа (беззнаковое).
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как int24 (дополнение до двух).
fn int24(raw: &str, i: usize) -> i32 {
    let w = word(raw, i);
    if w.bit(255) {
        -((!w).wrapping_add(U256::from(1u8)).to::<u64>() as i32)
    } else {
        w.to::<u64>() as i32
    }
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn squad() -> &'static families::Family {
    families::make("squadswap-v3-bsc", "v3_fork_pancake").unwrap()
}

/// Ответы полей пула в порядке field_calls (factory, token0, token1, fee, tickSpacing, lmPool[, метод ключа]).
fn fields(c: &Value, with_key: bool) -> Vec<Result<Bytes, String>> {
    let mut keys = vec![
        "factory",
        "token0",
        "token1",
        "fee",
        "tickSpacing",
        "lmPool",
    ];
    if with_key {
        keys.push("defaultFee");
    }
    keys.iter().map(|k| Ok(bytes(&c[*k]))).collect()
}

#[test]
fn registry_key_default_fee_takes_pool_and_without_setting_rejects() {
    let d = probe();
    let f = squad();
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let pool = addr(&p["pool"]);
        let fee = word(c["fee"].as_str().unwrap(), 0).to::<u32>();
        let df = word(c["defaultFee"].as_str().unwrap(), 0).to::<u32>();
        assert_ne!(
            fee, df,
            "{pool}: у записанных пулов своя комиссия менеджера"
        );
        // С настройкой: седьмой ответ — ключ реестра.
        let got = fields(c, true);
        let mut read = Read::default();
        fill(f, &mut read, &got).expect("поля пула по ABI PancakeSwap V3");
        assert_eq!(read.factory, addr(&d["factory"]));
        assert_eq!(read.fee, Some(fee), "fee_pips записи — fee()");
        let key = registry_key::from_answers(&got).unwrap();
        assert_eq!(key, Some(df));
        let call = IV3Factory::getPoolCall::abi_decode(&registry_call_with(f, &read, key)).unwrap();
        assert_eq!(call.fee.to::<u32>(), df);
        assert!(registered(pool, &bytes(&c["getPool_defaultFee"])).is_ok());
        // Без настройки: ответов шесть, ключ — fee(), реестр даёт 0 — отказ прежним текстом.
        let got = fields(c, false);
        let mut read = Read::default();
        fill(f, &mut read, &got).unwrap();
        let key = registry_key::from_answers(&got).unwrap();
        assert_eq!(key, None);
        let call = IV3Factory::getPoolCall::abi_decode(&registry_call_with(f, &read, key)).unwrap();
        assert_eq!(call.fee.to::<u32>(), fee);
        let why = registered(pool, &bytes(&c["getPool_fee"])).unwrap_err();
        assert_eq!(
            why,
            format!(
                "фабрика DEX не знает этот пул: её реестр по этим токенам даёт {}",
                Address::ZERO
            )
        );
    }
}

#[test]
fn fee_is_manager_get_fee_and_factory_manager_matches() {
    let d = probe();
    let pm = Address::from_slice(&hex(d["factory_calls"]["poolManager"].as_str().unwrap())[12..32]);
    assert_eq!(pm, addr(&d["manager"]));
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        assert_eq!(c["manager_getFee"], c["fee"], "{}", p["pool"]);
    }
}

#[test]
fn quoter_v2_answers_with_registry_key_only() {
    let d = probe();
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let fee = word(c["fee"].as_str().unwrap(), 0).to::<u32>();
        let df = word(c["defaultFee"].as_str().unwrap(), 0).to::<u32>();
        assert_eq!(registry_key::quoter_fee(true, Some(df), Some(fee)), Ok(df));
        for q in p["quoter"].as_array().unwrap() {
            assert!(
                q["quoterV2_defaultFee"].is_string(),
                "{}: QuoterV2 с defaultFee",
                p["pool"]
            );
        }
        for q in p["quoter_with_fee"].as_array().unwrap() {
            let e = &q["quoterV2_fee"]["error"]["message"];
            assert_eq!(
                e, "execution reverted",
                "{}: QuoterV2 с fee() откатывается",
                p["pool"]
            );
        }
    }
}

#[test]
fn local_math_equals_quoter_v2_and_virtual_quoter() {
    let d = probe();
    let (mut full, mut partial) = (0, Vec::new());
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let slot0 = c["slot0"].as_str().unwrap();
        let (sqrt, tick) = (word(slot0, 0), int24(slot0, 1));
        let liq = word(c["liquidity"].as_str().unwrap(), 0).to::<u128>();
        let fee = word(c["fee"].as_str().unwrap(), 0).to::<u32>();
        let spacing = word(c["tickSpacing"].as_str().unwrap(), 0).to::<u64>() as i32;
        let words = p["words"].as_array().unwrap();
        let ans: Vec<Bytes> = words.iter().map(|w| bytes(&w["raw"])).collect();
        let first = words.first().unwrap()["word"].as_i64().unwrap() as i16;
        let last = words.last().unwrap()["word"].as_i64().unwrap() as i16;
        let map = TickMap {
            spacing,
            ticks: v3_ticks(&ans).expect("тики TickLens").into(),
            words: (first, last),
        };
        let s = ClState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &map,
            version: Version::V3,
        };
        for q in p["quoter"].as_array().unwrap() {
            let qv2 = word(q["quoterV2_defaultFee"].as_str().unwrap(), 0);
            let v = q["virtual"].as_str().unwrap();
            let (used, out) = (word(v, 0), word(v, 1));
            assert_eq!(
                qv2, out,
                "{} ${}: QuoterV2 ≠ виртуальный квотер",
                p["pool"], q["usd"]
            );
            let amount = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = swap_cl::swap(&s, zfo, true, amount).expect("расчёт");
            let case = format!("{} {zfo} ${}", p["pool"], q["usd"]);
            if !r.complete || used != amount {
                // Цена ушла за записанные слова ±10 (или пул взял не весь вход): только «не больше квотера».
                assert!(
                    r.amount_out <= out,
                    "{case}: частичный выход больше квотера"
                );
                partial.push(case);
                continue;
            }
            full += 1;
            assert_eq!(r.amount_out, out, "{case}: математика ≠ QuoterV2");
        }
    }
    // 3 пула × 3 суммы × 2 стороны = 18.
    assert_eq!(full + partial.len(), 18);
    assert_eq!(full, FULL, "частичные: {partial:?}");
    assert_eq!(partial, PARTIAL);
}

/// Итог сверки на записи 15.11: Cake/WBNB и LTC/WBNB — все 12 случаев 1 в 1; USDT/WBNB (≈1,5 тыс. $, сторона USDT почти
/// пуста) — WBNB → USDT уже на 1 $ уводит цену за записанные слова ±10 (наш расчёт частичный, не больше квотера),
/// USDT → WBNB 1 и 100 $ — 1 в 1, на 10000 $ пул берёт не весь вход.
const FULL: usize = 14;
const PARTIAL: [&str; 4] = [
    "\"0xa95d87b443e1929465931611b7951b92c1746dae\" false $1",
    "\"0xa95d87b443e1929465931611b7951b92c1746dae\" false $100",
    "\"0xa95d87b443e1929465931611b7951b92c1746dae\" true $10000",
    "\"0xa95d87b443e1929465931611b7951b92c1746dae\" false $10000",
];

#[test]
fn fresh_fee_split_keeps_head_parse() {
    let d = probe();
    let f = squad();
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let head = vec![bytes(&c["slot0"]), bytes(&c["liquidity"]), bytes(&c["fee"])];
        let (rest, fee) = fee_fresh::split(true, &head).unwrap();
        assert_eq!(fee, Some(word(c["fee"].as_str().unwrap(), 0).to::<u32>()));
        let r = PoolRef::Contract(addr(&p["pool"]));
        let w = Want {
            family: f,
            reference: &r,
            spacing: Some(word(c["tickSpacing"].as_str().unwrap(), 0).to::<u64>() as i32),
            step: None,
            usdt: addr(&p["token1"]),
            token0: addr(&p["token0"]),
            token1: addr(&p["token1"]),
        };
        let parsed = v23::parse_head(&w, rest).expect("голова без fee()");
        assert!(
            matches!(parsed, PoolStateTick::Ticked(_, t) if t == int24(c["slot0"].as_str().unwrap(), 1))
        );
        // Без настройки ответы не трогаются.
        let (same, none) = fee_fresh::split(false, &head[..2]).unwrap();
        assert_eq!((same.len(), none), (2, None));
    }
}

#[test]
fn swap_log_is_pancake_v3_and_factory_has_no_swap_fee() {
    let d = probe();
    let logs = d["swap_logs"].as_array().unwrap();
    assert!(!logs.is_empty());
    for l in logs {
        let topics: Vec<B256> = l["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(topics[0], IPancakeV3Pool::Swap::SIGNATURE_HASH);
        let data = LogData::new_unchecked(topics, hex(l["data"].as_str().unwrap()).into());
        let e = IPancakeV3Pool::Swap::decode_log_data(&data).expect("Swap по ABI PancakeSwap V3");
        assert!(!e.sqrtPriceX96.is_zero());
    }
    let r = &d["factory_calls"]["swapFee(0x0,0)"];
    let answer = match r["result"].as_str() {
        Some(x) => Ok(Bytes::from(hex(x))),
        None => Err(r["error"]["message"].as_str().unwrap().to_string()),
    };
    assert!(
        !fee_probe::answers(&answer),
        "фабрика Squad не отвечает swapFee"
    );
}

#[test]
fn manager_event_topics() {
    let f = abi_squad::filters(&[(squad(), Address::ZERO, Address::ZERO)]);
    let t = f[0]["topics"][0].as_array().unwrap();
    assert_eq!(
        t[0].as_str().unwrap().parse::<B256>().unwrap(),
        keccak256("SetCustomFee(address,uint24)")
    );
    assert_eq!(
        t[1].as_str().unwrap().parse::<B256>().unwrap(),
        keccak256("ToggleCustomFee(address,bool)")
    );
    let u = f[1]["topics"][0].as_str().unwrap().parse::<B256>().unwrap();
    assert_eq!(u, keccak256("UpdatePoolManager(address)"));
}
