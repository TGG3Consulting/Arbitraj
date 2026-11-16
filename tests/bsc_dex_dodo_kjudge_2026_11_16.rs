//! Файл: tests/bsc_dex_dodo_kjudge_2026_11_16.rs
//! Что делает: проверки на записях с живого бота и своего узла BSC за 16.11.2026 (tests/data/dodo_kjudge_2026-11-16,
//! README там же): журнал KJUDGE_ERROR пулов DODO за сутки и сырые eth_call пяти пулов DODO на блоке 133993452.
//! Проверяется:
//! (1) журнал — все 493 записи dex.pool с одним текстом отката «execution reverted: KJUDGE_ERROR»; адресов пулов в
//!   текстах нет; Warn — первая неудача при приёме пачки (admit.rs «повтор на новых блоках»), Error — «за 6
//!   попыток» (reload.rs retry); в последнем приёме (20:26) — 21 пул: по Warn на пул (dodo-dpp USDC ×7, CAKE ×4 …);
//! (2) ответ узла с KJUDGE_ERROR — откат вызова (SwapReverted, errors.rs from_rpc), а не сбой связи узла
//!   (pool_fail::is_link = false) — при решении 339 пауза такого пула растёт (pool_fail.rs record); данные отката —
//!   Error(string) «KJUDGE_ERROR»;
//! (3) пять пулов на блоке H: состояние getPMMStateForCall (тот же селектор, что у бота, load_dodo.rs) разбирается
//!   нашим ABI; K и резервы в нём = _K_ / _BASE_RESERVE_ / _QUOTE_RESERVE_; все пять версий — из списка _kjudge
//!   FeeRateDIP3Impl (проверка K стоит), но ни у одного нет «K = 0 и нулевой резерв» — отката нет, getUserFeeRate(0)
//!   ответил; mtFeeRate пула = feeRateImpl.getFeeRate(пул, 0); version() у feeRateImpl — откат без данных;
//! (4) пул DPP ETH 0x238b… с резервами 0/0 (K ≠ 0) читается без ошибки, а наша математика (swap_dodo.rs) его не
//!   котирует ни в одну сторону (None — как откат контракта): пустой пул в сделки не идёт.
//! Чего здесь нет: сам откат KJUDGE_ERROR на записанном eth_call — на блоке H у снятых пулов его нет; адреса
//! откатывающихся пулов в записи не попали (промпт — docs/TASKS.md, t153 (б)).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (errors, pools::abi_dodo, pools::load_dodo, pools::pool_fail,
//! math::swap_dodo), arb::core::types::PoolRef, serde_json.
//! Настройки: нет (паузы 3000 / 600000 мс в (2) — советы docs/FIRST_RUN.md, здесь только как числа для расписания).

use std::time::Instant;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::errors::{Op, from_rpc};
use arb::venues::bsc_dex::math::swap_dodo::{PmmState, RState, sell_base_token, sell_quote_token};
use arb::venues::bsc_dex::pools::abi_dodo::IDodoPool;
use arb::venues::bsc_dex::pools::load_dodo::rstate;
use arb::venues::bsc_dex::pools::pool_fail::{Fails, is_link};

const DIR: &str = "tests/data/dodo_kjudge_2026-11-16";

/// Пять пулов записи (README): адрес и подпись «вид тикер».
const POOLS: [(&str, &str); 5] = [
    (
        "0xD5F05644EF5d0a36cA8C8B5177FfBd09eC63F92F",
        "dodo-dsp USDC",
    ),
    ("0x6098A5638d8D7e9Ed2f952d35B2b67c34EC6B476", "dodo-dpp BNB"),
    (
        "0x426808dd448d10eb7e4ae4b5dd4578aabbc30cd5",
        "dodo-dvm WBNB",
    ),
    (
        "0x9e6f399b98480f41c5502939d974906c8c4d3c06",
        "dodo-dpp CAKE",
    ),
    ("0x238b0b1366b83587675be8e2f5d02e669cbfd9e0", "dodo-dpp ETH"),
];

/// Селекторы записи (keccak256 сигнатур; первые два сверяются с нашим ABI в тесте).
const SEL_STATE: &str = "0xfd1ed7e9"; // getPMMStateForCall()
const SEL_FEE: &str = "0x44096609"; // getUserFeeRate(address)
const SEL_VERSION: &str = "0x54fd4d50"; // version()
const SEL_K: &str = "0xec2fd46d"; // _K_()
const SEL_BASE_RES: &str = "0x7d721504"; // _BASE_RESERVE_()
const SEL_QUOTE_RES: &str = "0xbbf5ce78"; // _QUOTE_RESERVE_()
const SEL_IMPL_FEE: &str = "0x848cc303"; // getFeeRate(address,address)

/// Версии пулов, у которых FeeRateDIP3Impl проверяет K (DODOEX/contractV2 2f1bcda, FeeRateDIP3Impl.sol L217-L219).
const KJUDGE_VERSIONS: [&str; 4] = ["DVM 1.0.2", "DSP 1.0.1", "DPP 1.0.0", "DPP Advanced 1.0.0"];

/// Запись из папки теста (JSON). Ошибок нет (паника — записи нет).
fn rec(name: &str) -> Value {
    let p = format!("{}/{DIR}/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Байты из hex-строки с «0x».
fn bytes(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).expect("hex")
}

/// Одно 32-байтовое слово ответа → U256.
fn word(s: &str) -> U256 {
    let b = bytes(s);
    assert_eq!(b.len(), 32, "ответ — одно слово");
    U256::from_be_slice(&b)
}

/// Строка ABI (смещение, длина, байты) из ответа без селектора.
fn abi_string(b: &[u8]) -> String {
    let len = U256::from_be_slice(&b[32..64]).to::<usize>();
    String::from_utf8(b[64..64 + len].to_vec()).expect("UTF-8")
}

/// Вызов eth_call записи: контракт (без учёта регистра), селектор и, если задано, подстрока аргументов.
fn call<'a>(calls: &'a [Value], to: &str, sel: &str, arg: Option<&str>) -> &'a Value {
    calls
        .iter()
        .find(|c| {
            let data = c["params"][0]["data"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            c["method"] == "eth_call"
                && c["params"][0]["to"]
                    .as_str()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(to)
                && data.starts_with(sel)
                && arg.is_none_or(|a| data.contains(a))
        })
        .expect("вызов есть в записи")
}

/// Поле result вызова (строка hex).
fn result(c: &Value) -> &str {
    c["result"].as_str().expect("ответ без ошибки")
}

/// Есть ли в тексте адрес (0x и ровно 40 hex-знаков).
fn has_address(text: &str) -> bool {
    text.match_indices("0x").any(|(i, _)| {
        let run = text[i + 2..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        run == 40
    })
}

/// Подпись записи журнала «вид тикер» (до первого двоеточия).
fn label(x: &Value) -> &str {
    let m = x["message"].as_str().expect("message");
    m.split(':').next().unwrap_or_default()
}

#[test]
fn journal_kjudge_records() {
    let v = rec("journal_entries_24h.json");
    let list = v.as_array().expect("массив записей");
    assert_eq!(list.len(), 493);
    for x in list {
        let m = x["message"].as_str().expect("message");
        assert_eq!(x["kind"], "dex.pool");
        assert!(m.contains("код 3: execution reverted: KJUDGE_ERROR"), "{m}");
        assert!(!has_address(m), "в тексте нет адреса пула: {m}");
        if x["level"] == "Warn" {
            assert!(m.contains("состояние пула не прочитано"), "{m}");
            assert!(m.ends_with("— повтор на новых блоках"), "{m}");
        } else {
            assert_eq!(x["level"], "Error");
            assert!(m.contains("так и не прочитано за 6 попыток"), "{m}");
        }
    }
    // Последний приём пачек (20:26–20:27): по одной записи Warn и Error на пул.
    let last: Vec<&Value> = list
        .iter()
        .filter(|x| x["at"].as_str().unwrap_or_default() >= "2026-11-16T20:26")
        .collect();
    let warn: Vec<&str> = last
        .iter()
        .filter(|x| x["level"] == "Warn")
        .map(|x| label(x))
        .collect();
    let errors = last.iter().filter(|x| x["level"] == "Error").count();
    assert_eq!((warn.len(), errors), (21, 21));
    let n = |l: &str| warn.iter().filter(|w| **w == l).count();
    assert_eq!(n("dodo-dpp USDC"), 7);
    assert_eq!(n("dodo-dpp CAKE"), 4);
    assert_eq!(n("dodo-dsp PORTO"), 2);
    for one in [
        "dodo-dsp USDC",
        "dodo-dsp BNB",
        "dodo-dpp ETH",
        "dodo-dpp BNB",
        "dodo-dvm USDC",
        "dodo-dvm BNB",
        "dodo-dpp DEXE",
        "dodo-dpp 1INCH",
    ] {
        assert_eq!(n(one), 1, "{one}");
    }
}

#[test]
fn kjudge_answer_is_pool_revert_not_node() {
    let v = rec("journal_entries_24h.json");
    let m = v[0]["message"].as_str().expect("message");
    // Текст узла: между «код 3: » и « — » (admit.rs: «{вид} {тикер}: … {причина} — повтор на новых блоках»).
    let node = m
        .split("код 3: ")
        .nth(1)
        .and_then(|s| s.split(" — ").next())
        .expect("текст узла");
    let e = from_rpc(3, node, "узел BSC (HTTP)", Op::Read, "");
    assert_eq!(e.code, ErrorCode::SwapReverted);
    assert!(!is_link(Some(e.code)), "откат пула — не сбой связи узла");
    // Данные отката — Error(string): селектор 0x08c379a0 и строка «KJUDGE_ERROR».
    let data = bytes(node.rsplit(": ").next().expect("данные отката"));
    assert_eq!(data[..4], [0x08u8, 0xc3, 0x79, 0xa0]);
    assert_eq!(abi_string(&data[4..]), "KJUDGE_ERROR");
    // Решение 339: неудачи пула с этим кодом удлиняют паузу (ключ — адрес пула записи; какой пул откатывался — не
    // известно, расписание от адреса не зависит).
    let r = PoolRef::Contract(POOLS[0].0.parse::<Address>().expect("адрес"));
    let f = Fails::default();
    let t0 = Instant::now();
    let why = (Some(e.code), e.message.as_str());
    let (a, fresh) = f.record(&r, why, 3_000, 600_000, t0);
    assert!(fresh);
    let (b, again) = f.record(&r, why, 3_000, 600_000, t0);
    assert!(!again, "та же причина — журнал не повторяется");
    assert_eq!((a.pause_ms, b.pause_ms), (3_000, 6_000));
}

#[test]
fn five_pools_read_without_kjudge() {
    let v = rec("raw_rpc.json");
    let calls = v.as_array().expect("массив вызовов");
    let hex4 = |s: [u8; 4]| format!("0x{}", alloy_primitives::hex::encode(s));
    assert_eq!(hex4(IDodoPool::getPMMStateForCallCall::SELECTOR), SEL_STATE);
    assert_eq!(hex4(IDodoPool::getUserFeeRateCall::SELECTOR), SEL_FEE);
    let impl_addr = "0xe5e9b0cab984b58b7e7ae17707d633295d5a4c4d";
    for (pool, name) in POOLS {
        let st = IDodoPool::getPMMStateForCallCall::abi_decode_returns(&bytes(result(call(
            calls, pool, SEL_STATE, None,
        ))))
        .expect("состояние по ABI");
        let k = word(result(call(calls, pool, SEL_K, None)));
        let b = word(result(call(calls, pool, SEL_BASE_RES, None)));
        let q = word(result(call(calls, pool, SEL_QUOTE_RES, None)));
        assert_eq!((st.K, st.B, st.Q), (k, b, q), "{name}");
        let version = abi_string(&bytes(result(call(calls, pool, SEL_VERSION, None))));
        assert!(
            KJUDGE_VERSIONS.contains(&version.as_str()),
            "{name}: {version}"
        );
        // Условие отката FeeRateDIP3Impl L123 не выполнено — и getUserFeeRate(0) ответил без отката.
        assert!(!(k.is_zero() && (b.is_zero() || q.is_zero())), "{name}");
        let fee = IDodoPool::getUserFeeRateCall::abi_decode_returns(&bytes(result(call(
            calls,
            pool,
            SEL_FEE,
            Some("0".repeat(64).as_str()),
        ))))
        .expect("комиссии по ABI");
        let arg = pool.trim_start_matches("0x").to_ascii_lowercase();
        let direct = word(result(call(
            calls,
            impl_addr,
            SEL_IMPL_FEE,
            Some(arg.as_str()),
        )));
        assert_eq!(
            fee.mtFeeRate, direct,
            "{name}: mtFeeRate = feeRateImpl.getFeeRate(пул, 0)"
        );
    }
    // version() у feeRateImpl — откат без данных (в FeeRateDIP3Impl 2f1bcda version() = «1.2.0», L221-L223).
    let ver = call(calls, impl_addr, SEL_VERSION, None);
    assert_eq!(ver["error"]["code"], 3);
    assert_eq!(ver["error"]["data"], "0x");
}

#[test]
fn empty_dpp_pool_is_read_but_never_quoted() {
    let v = rec("raw_rpc.json");
    let calls = v.as_array().expect("массив вызовов");
    let (pool, _) = POOLS[4];
    let st = IDodoPool::getPMMStateForCallCall::abi_decode_returns(&bytes(result(call(
        calls, pool, SEL_STATE, None,
    ))))
    .expect("состояние по ABI");
    assert!(st.B.is_zero() && st.Q.is_zero() && !st.K.is_zero());
    let s = PmmState {
        i: st.i,
        k: st.K,
        b: st.B,
        q: st.Q,
        b0: st.B0,
        q0: st.Q0,
        r: rstate(st.R.to::<u8>()).expect("R 0..=2"),
    };
    assert_eq!(s.r, RState::One);
    let one = U256::from(10u8).pow(U256::from(18u8));
    assert_eq!(
        sell_base_token(&s, one),
        None,
        "продажа базы — нет котировки"
    );
    assert_eq!(
        sell_quote_token(&s, one),
        None,
        "продажа котируемой — нет котировки"
    );
}
