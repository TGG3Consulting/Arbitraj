//! Файл: tests/bsc_dex_izi_live.rs
//! Что проверяет: решение 286 — iZiSwap (iziswap) и Biswap V3 (biswap_v3) на РЕАЛЬНЫХ ответах узла BSC, записанных на
//! блоке 133668333 (tests/data/izi_2026-11-15).
//! 1. Проверка пула (verify_izi::fields / registered / registry_call): записанные factory / tokenX / tokenY / fee и ответ
//!    реестра фабрики четырёх пулов трёх фабрик — пул принят; чужая фабрика, чужая монета, чужой ответ реестра —
//!    отказ; селектор реестра по виду (uint24 у iZiSwap, uint16 у Biswap V3).
//! 2. Ответ виртуального квотера quoteIzi (сырые слова) разбирается virtual_quoter::decode: выход = выходу пробного
//!    контракта; край пула (used < amountIn) — «не хватает ликвидности».
//! 3. Лесенка (load_izi::state + quote/pmm_ladder): в точках выход котировки = ответу пула; pool_token0 = tokenX.
//! 4. Шаг исполнителя: вид IZI = 14, данных нет, только v2; проба на цепи: исполнитель v2 шагом 14 получил ровно ответ
//!    квотера во всех 8 случаях; вид DEX — graphable, без своего пути, topic0 Swap — из записанных логов.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{verify_izi, load_izi, abi_izi, families, exec_callback, events},
//! exec::executor::kind, quote::{pmm_ladder, virtual_quoter}}.

use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;

use arb::core::types::{LadderPoint, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{abi_izi, events, exec_callback, families, load_izi, verify_izi};
use arb::venues::bsc_dex::quote::{pmm_ladder, virtual_quoter};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/izi_2026-11-15/");
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(s: &str) -> Address {
    s.parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

fn word(v: U256) -> Bytes {
    Bytes::from(v.to_be_bytes::<32>().to_vec())
}

fn word_addr(a: Address) -> Bytes {
    word(U256::from_be_slice(a.as_slice()))
}

fn kind_of(dex: &str) -> &'static str {
    if dex == "iziswap-bsc" {
        "iziswap"
    } else {
        "biswap_v3"
    }
}

#[test]
fn pool_verified_by_fields_and_registry() {
    let probe = json("probe.json");
    for p in probe["pools"].as_array().unwrap() {
        let dex = p["dex"].as_str().unwrap();
        let fam = families::make(dex, kind_of(dex)).expect("вид");
        let pool = addr(p["pool"].as_str().unwrap());
        let factory = addr(p["factory"].as_str().unwrap());
        let (x, y) = (
            addr(p["tokenX"].as_str().unwrap()),
            addr(p["tokenY"].as_str().unwrap()),
        );
        let fee = p["fee"].as_u64().unwrap();
        assert!(x < y, "tokenX < tokenY — фабрика сортирует пару");
        let answers = vec![
            Ok(word_addr(factory)),
            Ok(word_addr(x)),
            Ok(word_addr(y)),
            Ok(word(U256::from(fee))),
        ];
        let coin = if x == addr(USDT) { y } else { x };
        let cand = |coin: Address| Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().to_string(),
            coin,
            quote: addr(USDT),
        };
        let (found, f) = verify_izi::fields(&cand(coin), pool, factory, &answers).expect("поля");
        assert_eq!((found.token0, found.token1, f as u64), (x, y, fee));
        assert!(verify_izi::fields(&cand(pool), pool, factory, &answers).is_err());
        assert!(verify_izi::fields(&cand(coin), pool, pool, &answers).is_err());
        // Реестр: записанный ответ = пул; другой адрес — отказ.
        let reg = addr(p["registry_answer"].as_str().unwrap());
        assert!(verify_izi::registered(&found, pool, &word_addr(reg)).is_ok());
        assert!(verify_izi::registered(&found, pool, &word_addr(factory)).is_err());
        // Селектор реестра — по виду (чужой селектор на цепи откатывается, probe.json factories).
        let (to, data) =
            verify_izi::registry_call(fam.proto, factory, x, y, f).expect("вызов реестра");
        assert_eq!(to, factory);
        let sel = if fam.proto == Proto::BiswapV3 {
            [0x06, 0x68, 0xc8, 0xa2]
        } else {
            [0xbe, 0xcb, 0xcc, 0x6a]
        };
        assert_eq!(&data[..4], &sel);
        assert_eq!(U256::from_be_slice(&data[68..100]), U256::from(fee));
    }
}

#[test]
fn quoter_answers_decode_to_pool_output() {
    let probe = json("probe.json");
    let mut edges: Vec<(String, bool)> = Vec::new();
    // Ответы блока 133668333 и края пула в недостающие стороны (блок 133677207, ревью t118).
    let all = probe["quoteIzi_raw"]
        .as_array()
        .unwrap()
        .iter()
        .chain(probe["quoteIzi_raw_edges"]["answers"].as_array().unwrap());
    for q in all {
        let Some(raw) = q["raw"].as_str() else {
            assert!(q["error"].as_str().unwrap().contains("reverted"));
            continue;
        };
        let raw = alloy_primitives::hex::decode(raw).unwrap();
        let amount = dec(&q["amountIn"]);
        let out = dec(&q["probe_out"]);
        let paid = dec(&q["probe_paid"]);
        match virtual_quoter::decode(&raw, amount).expect("ответ по ABI") {
            Ok(v) => assert_eq!(v, out, "выход квотера = выходу пробного контракта"),
            Err(why) => {
                assert_eq!(why, virtual_quoter::SHORT);
                // Край пула: used — сколько пул взял на самом деле (= оплата пробного контракта).
                assert_eq!(U256::from_be_slice(&raw[..32]), paid);
                assert!(paid < amount);
                edges.push((
                    q["dex"].as_str().unwrap().to_string(),
                    q["x2y"].as_bool().unwrap(),
                ));
            }
        }
        // Вход вызова quoteIzi — селектор 0xc782d469.
        let pool = addr(q["pool"].as_str().unwrap());
        let d = virtual_quoter::data_izi(pool, q["x2y"].as_bool().unwrap(), amount);
        assert_eq!(&d[..4], &[0xc7, 0x82, 0xd4, 0x69]);
    }
    // Край пула записан в обе стороны у обоих видов (ревью t118): x2y и y2x — iZiSwap и Biswap V3.
    for want in [
        ("iziswap-bsc", true),
        ("iziswap-bsc", false),
        ("biswap-v3", true),
        ("biswap-v3", false),
        ("biswap-v3-1", true),
    ] {
        assert!(
            edges.iter().any(|(d, x)| d == want.0 && *x == want.1),
            "край {want:?}"
        );
    }
    assert_eq!(edges.len(), 5);
}

#[test]
fn ladder_points_are_pool_answers() {
    let probe = json("probe.json");
    let usdt = addr(USDT);
    // biswap-v3-1 USDT/WBNB: точка USDT → WBNB и WBNB → USDT (ответы квотера на блоке).
    let qs = probe["quoteIzi_raw"].as_array().unwrap();
    let pick = |i: usize| LadderPoint {
        amount_in: dec(&qs[i]["amountIn"]),
        amount_out: dec(&qs[i]["probe_out"]),
    };
    let wbnb = addr("0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c");
    let pair = (usdt, wbnb);
    let sqrt = U256::from(1u8) << 96;
    let state = load_izi::state(pair, wbnb, sqrt, vec![pick(4)], vec![pick(5)]);
    let PoolState::OraclePmm(d) = state else {
        panic!("не OraclePmm")
    };
    assert_eq!(d.pool_token0, Some(usdt), "tokenX = меньший адрес");
    assert!(!d.base_is_token0 && d.feasible);
    assert_eq!(
        pmm_ladder::out(&d.ladder_quote_in, pick(4).amount_in),
        Some(pick(4).amount_out)
    );
    assert_eq!(
        pmm_ladder::out(&d.ladder_base_in, pick(5).amount_in),
        Some(pick(5).amount_out)
    );
    assert_eq!(
        pmm_ladder::out(&d.ladder_quote_in, pick(4).amount_in + U256::from(1u8)),
        None
    );
    // Пул не инициализирован (sqrtPrice_96 = 0) — цена непригодна.
    let PoolState::OraclePmm(z) = load_izi::state(pair, wbnb, U256::ZERO, Vec::new(), Vec::new())
    else {
        panic!()
    };
    assert!(!z.feasible && z.price.is_zero());
}

#[test]
fn executor_step_and_family() {
    for (id, kind, proto) in [
        ("iziswap-bsc", "iziswap", Proto::IziSwap),
        ("biswap-v3", "biswap_v3", Proto::BiswapV3),
        ("biswap-v3-1", "biswap_v3", Proto::BiswapV3),
    ] {
        let fam = families::make(id, kind).expect("вид");
        assert!(fam.graphable && !fam.tradable && !fam.fork);
        assert_eq!(fam.proto, proto);
        assert_eq!(fam.executor_kind, Some(ExecKind::Izi));
        assert_eq!(abi_izi::topics(proto), events::topics(fam));
    }
    assert!(exec_callback::v2_only(ExecKind::Izi));
    assert_eq!(Params::Izi.kind(), 14);
    assert!(Params::Izi.data().is_empty());
    let sim = json("exec_sim.json");
    let probe = json("probe.json");
    let qs = probe["quoteIzi_raw"].as_array().unwrap();
    let ex = sim["execute"].as_array().unwrap();
    for (i, e) in ex.iter().take(8).enumerate() {
        assert_eq!(dec(&e["received"]), dec(&e["quoteIzi_out"]));
        assert_eq!(dec(&e["received"]), dec(&qs[i]["probe_out"]));
        assert_eq!(dec(&e["balance_in"]), dec(&qs[i]["amountIn"]));
    }
    assert!(ex[8]["revert"].as_str().unwrap().contains("reverted"));
    // topic0 Swap обоих видов — из записанных логов (probe.json event_topics).
    let izi_swap: B256 = "0x0fe977d619f8172f7fdbe8bb8928ef80952817d96936509f67d66346bc4cd10f"
        .parse()
        .unwrap();
    let bis_swap: B256 = "0xde449b421e7f751324933a2c4afee2ea35f7c7d2b6bdf310e7a7017b4d67bb91"
        .parse()
        .unwrap();
    let text = probe["event_topics"].to_string();
    assert!(text.contains(&format!("{izi_swap:#x}")) && text.contains(&format!("{bis_swap:#x}")));
    assert!(abi_izi::topics(Proto::IziSwap).contains(&izi_swap));
    assert!(abi_izi::topics(Proto::BiswapV3).contains(&bis_swap));
    assert!(!abi_izi::liquidity_topics(Proto::BiswapV3).contains(&bis_swap));
}

/// Ревью t118: fee пула Biswap V3 больше uint16 — вызова реестра нет, пул отклоняется с причиной; у iZiSwap (uint24) —
/// вызов строится. liquidity_topics отсекают Swap и Flash по значению topic0. Перечитывание каждым блоком: пул iZiSwap
/// без USDT — нет, с USDT — да; USDT не задан — нет.
#[test]
fn review_t118_rules() {
    use arb::core::types::PoolRef;
    use arb::venues::bsc_dex::pools::discover::Found;
    use arb::venues::bsc_dex::pools::per_block;
    let (f, x, y) = (
        Address::ZERO,
        addr(USDT),
        addr("0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c"),
    );
    let big = u32::from(u16::MAX) + 1;
    let err = verify_izi::registry_call(Proto::BiswapV3, f, x, y, big).unwrap_err();
    assert!(err.contains("uint16"), "{err}");
    assert!(verify_izi::registry_call(Proto::BiswapV3, f, x, y, u32::from(u16::MAX)).is_ok());
    assert!(verify_izi::registry_call(Proto::IziSwap, f, x, y, big).is_ok());
    let flash: B256 = "0xbdbdb71d7860376ba52b25a5028beea23581364a40522f6bcfb86bb1f2dca633"
        .parse()
        .unwrap();
    for p in [Proto::IziSwap, Proto::BiswapV3] {
        let liq = abi_izi::liquidity_topics(p);
        assert!(!liq.contains(&flash));
        assert_eq!(liq.len(), abi_izi::topics(p).len() - 2);
    }
    let fam = families::make("biswap-v3-1", "biswap_v3").expect("вид");
    let btcb = addr("0x7130d2a12b9bcbfae4f2634d864a1ee1ce3ead9c");
    let found = |t0: Address, t1: Address| Found {
        family: fam,
        reference: PoolRef::Contract(Address::ZERO),
        token0: t0,
        token1: t1,
        coin: t1,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    };
    assert!(per_block::each_block(&found(x, y), Some(x)));
    assert!(
        !per_block::each_block(&found(btcb, y), Some(x)),
        "пул без USDT — не каждым блоком"
    );
    assert!(!per_block::each_block(&found(x, y), None));
    let metric = families::make("metric-bsc", "metric_omm").expect("вид");
    let mut m = found(btcb, y);
    m.family = metric;
    assert!(
        per_block::each_block(&m, Some(x)),
        "Metric — как раньше (needs)"
    );
}
