//! Файл: tests/bsc_dex_stable_list_live.rs
//! Что делает: решение 292 на ЖИВЫХ записях узла BSC (блок 133724547, 15.11.2026; tests/data/nerve_2026-11-15,
//! tests/data/ellipsis_2026-11-15) — без сети и без моков:
//! (а) источник пулов из dex.stable_pools (pools/stable_list.rs): вызовы «монеты пула» совпадают байт в байт с
//!     записанными запросами; по записанным ответам getToken(k) (Nerve 3pool) и coins(k) (Ellipsis 3pool) кандидаты
//!     строятся для пар (BUSD, USDT) и (USDC, USDT), для настоящей монеты, которой в пуле нет (BTCB), — не строятся;
//! (б) своя математика вида на записанном состоянии = записанному квотеру пула: Saddle (A_PRECISION = 100, 256
//!     раундов, откат — как собирает load_saddle.rs) = calculateSwap; Curve 2020 (сырой A, 255 раундов — как
//!     load_curve.rs) = get_dy; по 3 суммы на каждую из 6 сторон.
//! Кто вызывает: cargo test --test bsc_dex_stable_list_live.
//! От чего зависит: arb::venues::bsc_dex::pools (stable_list, families::Proto, abi_saddle), math::stableswap_v1.

use std::collections::HashMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::math::stableswap_v1::{self, Template, curve2020};
use arb::venues::bsc_dex::pools::abi_saddle::ISaddleSwap;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::pools::stable_list;

/// Запись из tests/data/<dir>/<file>. Принимает: папку и файл. Возвращает: JSON. Паника — нет файла (тест падает).
fn rec(dir: &str, file: &str) -> Value {
    let p = format!("{}/tests/data/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("запись")).expect("JSON")
}

/// Ответ вызова по имени: Ok(байты) или Err(текст отката). Паника — нет вызова в записи.
fn answer(probe: &Value, name: &str) -> Result<Bytes, String> {
    let c = probe["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("нет вызова {name}"));
    match c["response"]["result"].as_str() {
        Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
        None => Err(c["response"]["error"]["message"].to_string()),
    }
}

/// Данные запроса вызова по имени (hex). Паника — нет вызова.
fn request(probe: &Value, name: &str) -> String {
    probe["calls"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["request"]["params"][0]["data"]
        .as_str()
        .unwrap()
        .to_lowercase()
}

/// Слово ответа → U256. Паника — откат.
fn word(probe: &Value, name: &str) -> U256 {
    U256::from_be_slice(&answer(probe, name).expect("ответ")[..32])
}

/// Монеты записи: тикер → адрес.
fn coin(coins: &Value, sym: &str) -> Address {
    Address::from_str(coins[sym].as_str().unwrap()).unwrap()
}

/// (а) для пула: сверка запросов и кандидаты. Принимает: папку, вид, имя вызова «монета k» в записи.
fn candidates_case(dir: &str, proto: Proto, getter: &str) {
    let probe = rec(dir, "probe.json");
    let coins = rec(dir, "coins.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let calls = stable_list::token_calls(proto, pool);
    // Записаны индексы 0..=3 (3 — откат за последней монетой); дальше tokens() не смотрит (до первого отката).
    let mut answers = Vec::new();
    for (k, (to, data)) in calls.iter().take(4).enumerate() {
        let name = format!("{getter}({k})");
        assert_eq!(*to, pool);
        assert_eq!(
            format!("0x{}", alloy_primitives::hex::encode(data)),
            request(&probe, &name)
        );
        answers.push(answer(&probe, &name));
    }
    assert!(answers[3].is_err(), "индекс 3 должен откатиться (3 монеты)");
    let tokens = stable_list::tokens(proto, &answers).unwrap();
    let (busd, usdc, usdt, btcb) = (
        coin(&coins, "BUSD"),
        coin(&coins, "USDC"),
        coin(&coins, "USDT"),
        coin(&coins, "BTCB"),
    );
    assert_eq!(tokens.len(), 3);
    let subs: HashMap<Address, String> = [(busd, "BUSD"), (usdc, "USDC"), (btcb, "BTCB")]
        .into_iter()
        .map(|(a, s)| (a, s.to_string()))
        .collect();
    let list = stable_list::candidates("x", pool, &tokens, &subs, &[usdt]);
    let got: Vec<(Address, Address)> = list.iter().map(|p| (p.base, p.quote)).collect();
    assert!(
        got.contains(&(busd, usdt)),
        "{dir}: нет кандидата BUSD/USDT"
    );
    assert!(
        got.contains(&(usdc, usdt)),
        "{dir}: нет кандидата USDC/USDT"
    );
    assert!(
        got.iter().all(|(c, _)| *c != btcb),
        "{dir}: BTCB нет в пуле"
    );
    assert_eq!(got.len(), 2);
    assert!(list.iter().all(|p| p.pool == format!("{pool:#x}")));
    // Пара с монетой не из пула не строится и как встречный токен.
    assert!(stable_list::pairs(&tokens, &[busd], &[btcb]).is_empty());
    assert_eq!(
        stable_list::pairs(&tokens, &[usdt], &[busd]),
        vec![(usdt, busd)]
    );
}

#[test]
fn nerve_candidates_from_live_get_token() {
    candidates_case("nerve_2026-11-15", Proto::SaddleStable, "nerve getToken");
}

#[test]
fn ellipsis_candidates_from_live_coins() {
    candidates_case("ellipsis_2026-11-15", Proto::CurveStable, "ellipsis coins");
}

/// (б) математика вида = квотер пула на записи. Принимает: папку, префикс имён, имена [балансы, монета k, квотер],
/// шаблон, A, комиссию. Возвращает: число совпавших котировок.
fn math_case(dir: &str, pre: &str, names: [&str; 3], t: &Template, amp: U256, fee: U256) -> usize {
    let [bal, getter, q] = names;
    let probe = rec(dir, "probe.json");
    let coins = rec(dir, "coins.json");
    let balances: Vec<U256> = (0..3)
        .map(|k| word(&probe, &format!("{pre} {bal}({k})")))
        .collect();
    // decimals() каждой монеты пула по её адресу (ответ «монета k» — адрес в слове) → множитель шаблона.
    let rates: Vec<U256> = (0..3)
        .map(|k| {
            let raw = answer(&probe, &format!("{pre} {getter}({k})")).unwrap();
            let tok = Address::from_slice(&raw[12..32]);
            let addr = coins
                .as_object()
                .unwrap()
                .values()
                .filter_map(|v| v.as_str())
                .find(|s| Address::from_str(s).ok() == Some(tok))
                .unwrap();
            let d = word(&probe, &format!("decimals() {addr}"));
            stableswap_v1::rate_for_decimals(d.to::<u8>()).unwrap()
        })
        .collect();
    let mut n = 0;
    for c in probe["calls"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let Some(args) = name.strip_prefix(&format!("{pre} {q}(")) else {
            continue;
        };
        let v: Vec<&str> = args.trim_end_matches(')').split(',').collect();
        let (i, j): (usize, usize) = (v[0].parse().unwrap(), v[1].parse().unwrap());
        let dx: U256 = v[2].parse().unwrap();
        let got = stableswap_v1::exchange_out_t(i, j, dx, &balances, &rates, amp, fee, t).unwrap();
        assert_eq!(got, word(&probe, name), "{name}");
        n += 1;
    }
    n
}

#[test]
fn nerve_math_matches_live_calculate_swap() {
    let probe = rec("nerve_2026-11-15", "probe.json");
    let ss = ISaddleSwap::swapStorageCall::abi_decode_returns(
        &answer(&probe, "nerve swapStorage()").unwrap(),
    )
    .unwrap();
    assert_eq!(word(&probe, "nerve paused()"), U256::ZERO);
    // Шаблон Saddle — как load_saddle.rs: A_PRECISION = 100, 256 раундов, откат при несходимости.
    let t = Template {
        amp_precision: U256::from(100u8),
        max_rounds: 256,
        strict: true,
    };
    let amp = word(&probe, "nerve getAPrecise()");
    let names = ["getTokenBalance", "getToken", "calculateSwap"];
    let n = math_case("nerve_2026-11-15", "nerve", names, &t, amp, ss.swapFee);
    assert_eq!(n, 18);
}

#[test]
fn ellipsis_math_matches_live_get_dy() {
    let probe = rec("ellipsis_2026-11-15", "probe.json");
    let (amp, fee) = (word(&probe, "ellipsis A()"), word(&probe, "ellipsis fee()"));
    let names = ["balances", "coins", "get_dy"];
    let n = math_case(
        "ellipsis_2026-11-15",
        "ellipsis",
        names,
        &curve2020(),
        amp,
        fee,
    );
    assert_eq!(n, 18);
}
