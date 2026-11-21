//! Файл: tests/arbitrum_forks_482_reg_live.rs
//! Что делает: решение 482 (поручение координатора) — источники пулов без индексатора:
//! (1) подписи событий создания (без записи): topic0 LBPairCreated и PoolCreated (Metric), как их собирает
//! alloy::sol! из abi_lb.rs / abi_metric.rs, = keccak256 канонической подписи из исходников Sourcify 42161;
//! (2) Swapline V1 (lb_v21) на записи своего узла: ответ getAllLBPairs(tokenX, tokenY) фабрики (вопрос factory_ask.rs
//! Q::Lb) содержит каждую пару перечисления getLBPairAtIndex, getFactory() пары = фабрика;
//! (3) Metric V1 (metric_omm): nextPoolIdx / idxToPool / getImmutables разбирает metric_list.rs, каждый пул проходит
//! проверку verify_metric::decide (isPool, фабрика, токены); в коде пула есть topic0 событий abi_metric::topics()
//! (записаны на BSC — бот слушает их и на Arbitrum);
//! (4) логи событий создания за окно записи (если есть): created_log::candidate даёт пул перечисления.
//! Кто вызывает: cargo test. Пункты 2–4 — #[ignore] до записи (tools/arbitrum_record/forks_482.py → reg.json).
//! От чего зависит: tests/data/arbitrum_forks_482_<дата>/reg.json, arb::venues::bsc_dex::pools::{abi_lb, abi_metric,
//! created_log, factory_ask, families, metric_list, verify, verify_metric}.

use std::path::PathBuf;

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, Bytes, hex, keccak256};
use serde_json::Value;

use arb::venues::bsc_dex::pools::abi_lb::ILBFactory;
use arb::venues::bsc_dex::pools::abi_metric::{self, IMetricOmmFactory};
use arb::venues::bsc_dex::pools::factory_ask::{Q, decode_q};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{created_log, families, metric_list, verify_metric};

/// Каноническая подпись LBPairCreated — ILBFactory.sol (lfj-gg/joe-v2 v2.1.0 = Sourcify LBFactory Swapline).
const LB_CREATED: &str = "LBPairCreated(address,address,uint256,address,uint256)";
/// Каноническая подпись PoolCreated — IMetricOmmPoolFactory.sol (Sourcify 42161 0x2a53…8f32; ExtensionOrders —
/// PoolExtensionsConfig.sol, шесть uint256).
const METRIC_CREATED: &str = concat!(
    "PoolCreated(address,address,address,uint256,address,address,address,address[],",
    "(uint256,uint256,uint256,uint256,uint256,uint256),uint256,uint256,uint256,uint256,uint24,",
    "uint24,uint24,uint24,address,int24,uint256[],uint256[])"
);

#[test]
fn created_topics_match_source_signatures() {
    assert_eq!(
        ILBFactory::LBPairCreated::SIGNATURE_HASH,
        keccak256(LB_CREATED)
    );
    assert_eq!(
        IMetricOmmFactory::PoolCreated::SIGNATURE_HASH,
        keccak256(METRIC_CREATED)
    );
}

/// reg.json последней записи tests/data/arbitrum_forks_482_*. Паника — записи нет.
fn reg() -> Value {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_forks_482_"))
        })
        .collect();
    dirs.sort();
    let dir = dirs
        .pop()
        .expect("нет записи: tools/arbitrum_record/forks_482.py");
    let text = std::fs::read_to_string(dir.join("reg.json")).expect("reg.json");
    let v: Value = serde_json::from_str(&text).expect("json");
    assert!(
        v.get("error").is_none(),
        "запись reg.json с ошибкой: {}",
        v["error"]
    );
    v
}

/// Ответ узла {"result": "0x…"} → байты; откат / нет ответа — None.
fn res(v: &Value) -> Option<Bytes> {
    let h = v.get("result")?.as_str()?;
    Some(Bytes::from(hex::decode(h.trim_start_matches("0x")).ok()?))
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Адрес из ответа одним словом.
fn addr_of(b: &Bytes) -> Address {
    Address::from_slice(&b[12..32])
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/forks_482.py (reg.json)"]
fn swapline_get_all_lb_pairs_finds_every_pair() {
    let r = reg();
    let factory = addr(&r["swapline_factory"]);
    let pairs = r["swapline"]["pairs"].as_array().unwrap();
    assert!(!pairs.is_empty(), "перечисление фабрики Swapline пусто");
    for p in pairs {
        let pair = addr(&p["pair"]);
        assert_eq!(
            addr_of(&res(&p["getFactory"]).unwrap()),
            factory,
            "{pair}: getFactory"
        );
        let raw = res(&p["getAllLBPairs"]).expect("getAllLBPairs");
        let found = decode_q(Q::Lb, &raw).expect("getAllLBPairs по ABI");
        assert!(
            found.contains(&pair),
            "{pair}: нет в getAllLBPairs ({found:?})"
        );
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/forks_482.py (reg.json)"]
fn metric_enumeration_and_verify() {
    let r = reg();
    let factory = addr(&r["metric_factory"]);
    let m = &r["metric"];
    let n = metric_list::count(&res(&m["nextPoolIdx"]).unwrap()).unwrap();
    let pools = m["pools"].as_array().unwrap();
    assert_eq!(
        pools.len() as u64,
        n.saturating_sub(1),
        "idxToPool(1..n − 1)"
    );
    let f = families::make("metric-v1@arbitrum", "metric_omm").unwrap();
    for p in pools {
        let pool = metric_list::pool_of(&res(&p["idxToPool"]).unwrap()).unwrap();
        assert_eq!(pool, addr(&p["pool"]));
        let imm = res(&p["getImmutables"]).expect("getImmutables");
        let (fa, t0, t1) = metric_list::tokens_of(&imm).expect("getImmutables по ABI");
        assert_eq!(fa, factory, "{pool}: фабрика пула");
        let c = Candidate {
            family: f,
            pool: format!("{pool:#x}"),
            coin: t0,
            quote: t1,
        };
        let answers = vec![Ok(res(&p["isPool"]).unwrap()), Ok(imm)];
        verify_metric::decide(&c, pool, factory, &answers).expect("проверка пула");
    }
    // topic0 событий пула, которые бот слушает (записаны на BSC), — в коде пула Arbitrum (PUSH32 + topic).
    let code = hex::decode(m["code"]["code"].as_str().unwrap().trim_start_matches("0x")).unwrap();
    for t in abi_metric::topics() {
        let mut needle = vec![0x7f_u8];
        needle.extend_from_slice(t.as_slice());
        let has = code.windows(33).any(|w| w == needle.as_slice());
        assert!(
            has,
            "topic0 {t} события пула Metric BSC нет в коде пула Arbitrum"
        );
    }
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/forks_482.py (reg.json)"]
fn created_logs_give_enumerated_pools() {
    let r = reg();
    // (запись, id DEX, вид, номер темы встречного токена: LBPairCreated — tokenY, PoolCreated Metric — token1).
    let cases = [
        ("swapline", "swapline-v1@arbitrum", "lb_v21", 2),
        ("metric", "metric-v1@arbitrum", "metric_omm", 3),
    ];
    for (name, id, kind, at) in cases {
        let f = families::make(id, kind).unwrap();
        let logs = r["logs"][name]["logs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let (list, field) = match name {
            "swapline" => (&r["swapline"]["pairs"], "pair"),
            _ => (&r["metric"]["pools"], "pool"),
        };
        for raw in logs {
            let log: Log = serde_json::from_value(raw).expect("лог");
            let quote = Address::from_word(log.topics()[at]);
            let c = created_log::candidate(f, &log, quote).expect("событие создания");
            let known = list.as_array().unwrap().iter().any(|p| p[field] == c.pool);
            assert!(
                known,
                "{name}: пул события {} не найден перечислением",
                c.pool
            );
        }
    }
}
