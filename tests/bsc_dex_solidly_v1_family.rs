//! Файл: tests/bsc_dex_solidly_v1_family.rs
//! Что делает: семейство Solidly V1 — THENA V1 (решение 122, часть F) без сети и без моков:
//! (1) таблица сопоставления dex.kind = solidly_v1 → Proto::SolidlyV1, в графе, без своего пути исполнения, вид шага
//!   исполнителя SOLIDLY; нужна только фабрика; без exec.executor_address — не исполнимо через исполнитель (сигнал);
//! (2) вопросы к фабрике при поиске пулов (factory_ask.rs): данные вызова = селектор официальной сигнатуры + аргументы
//!   ABI (getPair(address,address,bool) THENA V1, getDODOPoolBidirection DODO V2);
//! (3) сверка своей математики Pair.sol с ответом самого пула getAmountOut на ЖИВОЙ записи узла — тест ждёт записи
//!   tests/data/thena_v1/responses.json (запрос записи — в tests/README.md); пока записи нет — помечен #[ignore].
//! Кто вызывает: cargo test (третий — cargo test --test bsc_dex_solidly_v1_family -- --ignored, когда запись есть).
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, factory_ask, abi_solidly, math::solidly_v1_math),
//! arb::settings (настоящая SQLite во временной папке).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, keccak256};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::solidly_v1_math;
use arb::venues::bsc_dex::pools::abi_solidly::ISolidlyPool;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::factory_ask::{Q, encode_q};
use arb::venues::bsc_dex::pools::families::{self, Proto};

/// Адреса монет BSC — как у BingX (tests/data/indexers_2026-10-24/README.md): CAKE и USDT.
const CAKE: &str = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82";
const USDT: &str = "0x55d398326f99059fF775485246999027B3197955";

fn a(s: &str) -> Address {
    s.parse().unwrap()
}

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-solv1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// solidly_v1: в графе, без своего пути (tradable=false), шаг SOLIDLY; только фабрика; без адреса исполнителя — сигнал.
#[tokio::test]
async fn solidly_v1_family_requirements() {
    let c = ctx("req").await;
    let id = "thena";
    let s = Scope::Dex(id.into());
    let f = families::make(id, "solidly_v1").unwrap();
    assert_eq!(f.proto, Proto::SolidlyV1);
    assert!(f.proto.is_solidly());
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
    assert_eq!(f.executor_kind, Some(ExecKind::Solidly));
    assert_eq!(families::required(&c, f), vec!["dex.factory"]);
    assert!(
        !exec_kind::via_executor(&c, f),
        "без exec.executor_address — только сигнал"
    );
    for (n, v) in [
        ("dex.kind", json!("solidly_v1")),
        ("dex.pool_enabled", json!(true)),
        (
            "dex.factory",
            json!("0x1111111111111111111111111111111111111111"),
        ),
    ] {
        c.settings.set(n, s.clone(), Some(v), "тест").await.unwrap();
    }
    let f = families::classify(&c, id).unwrap();
    assert_eq!(f.proto, Proto::SolidlyV1);
    // Виды шагов остальных семейств части F.
    let kind = |k: &str| families::make("x", k).unwrap().executor_kind;
    assert_eq!(kind("slipstream"), Some(ExecKind::V3));
    assert_eq!(kind("thena_fusion"), Some(ExecKind::Algebra));
    assert_eq!(kind("v3_fork"), Some(ExecKind::V3));
    assert_eq!(kind("dodo_dsp"), Some(ExecKind::Dodo));
    assert_eq!(kind("pancakeswap_infinity_cl"), None);
}

/// Данные вызова = keccak-селектор официальной сигнатуры + слова ABI (адрес — 32 байта, bool — 32 байта).
#[test]
fn factory_questions_encode_by_official_signatures() {
    let word = |x: Address| x.into_word().0.to_vec();
    let flag = |b: bool| U256::from(b as u8).to_be_bytes::<32>().to_vec();
    let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
    let mut want = sel("getPair(address,address,bool)");
    want.extend(word(a(CAKE)));
    want.extend(word(a(USDT)));
    want.extend(flag(true));
    let got = encode_q(
        Q::Solidly {
            v1: true,
            stable: true,
        },
        a(CAKE),
        a(USDT),
    );
    assert_eq!(got.to_vec(), want);
    let mut want = sel("getPool(address,address,bool)");
    want.extend(word(a(CAKE)));
    want.extend(word(a(USDT)));
    want.extend(flag(false));
    let got = encode_q(
        Q::Solidly {
            v1: false,
            stable: false,
        },
        a(CAKE),
        a(USDT),
    );
    assert_eq!(got.to_vec(), want);
    let mut want = sel("getDODOPoolBidirection(address,address)");
    want.extend(word(a(CAKE)));
    want.extend(word(a(USDT)));
    assert_eq!(encode_q(Q::Dodo, a(CAKE), a(USDT)).to_vec(), want);
    let mut want = sel("poolByPair(address,address)");
    want.extend(word(a(CAKE)));
    want.extend(word(a(USDT)));
    assert_eq!(encode_q(Q::Algebra, a(CAKE), a(USDT)).to_vec(), want);
}

/// Слово hex-ответа eth_call (0x + 64·n знаков) → байты. Паника — запись испорчена.
fn hex(v: &Value) -> Vec<u8> {
    let s = v.as_str().expect("result — строка");
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).expect("hex")
}

/// Своя математика Pair.sol (THENA V1) = ответ самого пула getAmountOut на одном блоке. Формат записи
/// tests/data/thena_v1/responses.json: {block, pools: [{pool, metadata: <result metadata()>, fee: <result
/// getFee(stable) фабрики>, quotes: [{amount_in: "<десятичное>", token_in: "0x…", out: <result getAmountOut>}]}]}.
#[test]
#[ignore = "нет записи tests/data/thena_v1/responses.json — запрос записи в tests/README.md (решение 122 F)"]
fn thena_v1_math_matches_live_get_amount_out() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/thena_v1/responses.json"
    );
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    let pools = rec["pools"].as_array().expect("pools");
    assert!(!pools.is_empty());
    for p in pools {
        let m = ISolidlyPool::metadataCall::abi_decode_returns(&hex(&p["metadata"])).unwrap();
        let fee = U256::from_be_slice(&hex(&p["fee"]));
        for q in p["quotes"].as_array().expect("quotes") {
            let amount: U256 = q["amount_in"].as_str().unwrap().parse().unwrap();
            let zero = a(q["token_in"].as_str().unwrap()) == m.t0;
            let (ri, ro, di, dout) = if zero {
                (m.r0, m.r1, m.dec0, m.dec1)
            } else {
                (m.r1, m.r0, m.dec1, m.dec0)
            };
            let want = U256::from_be_slice(&hex(&q["out"]));
            let got = if m.st {
                solidly_v1_math::stable_amount_out(amount, ri, ro, di, dout, fee)
            } else {
                solidly_v1_math::volatile_amount_out(amount, ri, ro, fee)
            };
            assert_eq!(got, Some(want), "пул {} вход {amount}", p["pool"]);
        }
    }
}
