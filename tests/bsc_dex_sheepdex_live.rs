//! Файл: tests/bsc_dex_sheepdex_live.rs
//! Что проверяет: решение 274 — SheepDex (v3_fork + dex.v3_fee_source = factory_swap_fee) на РЕАЛЬНЫХ ответах узла BSC,
//! записанных на одном блоке 133593936 (tests/data/sheepdex_2026-11-14/probe.json): пулы BTCB/BUSD 0x1978…5f62 и
//! USDT/WBNB 0x187f…4065.
//! 1. Пул — это пул фабрики SpeFactory: factory() = 0x5715…, getPool(token0, token1, fee) = сам пул.
//! 2. swapFee(адрес, 0) фабрики для нулевого адреса и адреса виртуального квотера = 0 — разбор fee_override::check: Ok.
//! 3. Тики из ответов Uniswap TickLens (тот же разбор, что у загрузчика, load_batch_v2v3::v3_ticks) + slot0/liquidity
//!    и fee() пула → наша математика V3 (math/swap_cl.rs) даёт ТОТ ЖЕ выход, что виртуальный квотер с обратным вызовом
//!    swapCallback (новый артефакт contracts/out/VirtualQuoter.bin-runtime через state override, настоящий swap пула) —
//!    на 1, 100 и 10000 долларов, в обе стороны (10 случаев из 12; два случая на 10000 $ уходят за записанные слова
//!    битмапа ±6 — наша математика честно частичная, её выход не больше квотера).
//! 4. Реальные логи Swap пулов — topic0 и данные события Swap Uniswap V3 (7 полей) — разбираются ABI IUniswapV3Pool.
//! 5. Защита от забытой dex.v3_fee_source (fee_probe::answers) на ответах блока 133600177
//!    (tests/data/sheepdex_2026-11-14/swap_fee_probe.json): фабрика SheepDex на swapFee(0x0, 0) и (0x0, 500) отвечает
//!    uint24 — «отвечает»; фабрики Uniswap V3 и PancakeSwap V3 BSC откатываются — «не отвечает».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{math::swap_cl, pools::{abi_v2v3, fee_override, fee_probe, load_batch_v2v3,
//! ticks}}.

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, Bytes, LogData, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::swap_cl::{self, ClState, Version};
use arb::venues::bsc_dex::pools::abi_v2v3::IUniswapV3Pool;
use arb::venues::bsc_dex::pools::fee_override;
use arb::venues::bsc_dex::pools::fee_probe;
use arb::venues::bsc_dex::pools::load_batch_v2v3::v3_ticks;
use arb::venues::bsc_dex::pools::ticks::TickMap;

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/sheepdex_2026-11-14/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
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

fn addr(raw: &str) -> Address {
    Address::from_slice(&hex(raw)[12..32])
}

/// Ответ JSON-RPC eth_call как у узла: result — ответ, error — откат с текстом.
fn rpc_answer(v: &Value) -> Result<Bytes, String> {
    match v["result"].as_str() {
        Some(r) => Ok(Bytes::from(hex(r))),
        None => Err(v["error"]["message"].as_str().unwrap().to_string()),
    }
}

#[test]
fn swap_fee_probe_tells_sheepdex_from_uniswap_and_pancake() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/sheepdex_2026-11-14/swap_fee_probe.json"
    );
    let d: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let sf = &d["swap_fee"];
    for (dex, want) in [
        ("sheepdex", true),
        ("uniswap_bsc", false),
        ("pancakeswap_v3_bsc", false),
    ] {
        for k in ["swapFee(0x0,0)", "swapFee(0x0,500)"] {
            let got = fee_probe::answers(&rpc_answer(&sf[dex][k]));
            assert_eq!(got, want, "{dex} {k}");
        }
    }
}

#[test]
fn pools_are_factory_pools_and_no_own_fee() {
    let d = probe();
    let factory: Address = d["factory"].as_str().unwrap().parse().unwrap();
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        assert_eq!(addr(c["factory"].as_str().unwrap()), factory);
        assert_eq!(addr(c["getPool"].as_str().unwrap()), pool);
        let answers: Vec<Bytes> = ["swapFee_zero_addr", "swapFee_vq"]
            .iter()
            .map(|k| Bytes::from(hex(c[*k].as_str().unwrap())))
            .collect();
        assert!(fee_override::check("sheepdex", &answers).is_ok());
    }
}

#[test]
fn local_math_equals_virtual_quoter_swap_callback() {
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
        let ans: Vec<Bytes> = words
            .iter()
            .map(|w| Bytes::from(hex(w["raw"].as_str().unwrap())))
            .collect();
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
            let raw = q["raw"].as_str().unwrap();
            let (used, out) = (word(raw, 0), word(raw, 1));
            let amount = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
            assert_eq!(used, amount, "{}: квотер взял не весь вход", p["pool"]);
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = swap_cl::swap(&s, zfo, true, amount).expect("расчёт");
            // Записаны слова битмапа ±6 от текущего. Если цена уходит за них (тонкий пул, крупная сумма), наша
            // математика честно останавливается (complete = false — как и у бота при той же карте тиков), а квотер
            // на цепи идёт дальше: сравнивать выход нельзя, только «частичный не больше полного».
            if !r.complete {
                assert!(
                    r.amount_out <= out,
                    "{} {zfo} ${}: частичный выход больше квотера",
                    p["pool"],
                    q["usd"]
                );
                partial.push(format!("{} {zfo} ${}", p["pool"], q["usd"]));
                continue;
            }
            full += 1;
            assert_eq!(
                r.amount_out, out,
                "{} zeroForOne={zfo} ${}: математика ≠ виртуальный квотер",
                p["pool"], q["usd"]
            );
        }
    }
    // 2 пула × 3 суммы × 2 стороны = 12. На записи 14.11 за слова ±6 уходят ровно два случая на 10000 $ (тонкие пулы:
    // BUSD → BTCB и USDT → WBNB); остальные 10 совпадают с квотером 1 в 1.
    assert_eq!(full, 10);
    assert_eq!(
        partial,
        [
            "\"0x197880438436c426392570cb4782432ec9135f62\" false $10000",
            "\"0x187f9b3aa52f3ce82334bea80ffe769e1c134065\" true $10000",
        ]
    );
}

#[test]
fn swap_logs_decode_as_uniswap_v3() {
    let d = probe();
    let logs = d["logs"].as_array().unwrap();
    assert!(!logs.is_empty());
    for l in logs {
        let topics: Vec<B256> = l["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(topics[0], IUniswapV3Pool::Swap::SIGNATURE_HASH);
        let data = LogData::new_unchecked(topics, hex(l["data"].as_str().unwrap()).into());
        let e = IUniswapV3Pool::Swap::decode_log_data(&data).expect("Swap по ABI Uniswap V3");
        assert!(!e.sqrtPriceX96.is_zero(), "цена после обмена");
    }
}
