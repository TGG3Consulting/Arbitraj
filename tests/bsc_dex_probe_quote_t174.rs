//! Файл: tests/bsc_dex_probe_quote_t174.rs
//! Что проверяет: решение 388 (задача t174) — проба налогов берёт «ожидалось без налога» виртуальным квотером у DEX с
//! dex.quoter_virtual = да (quote/probe_quote.rs), а не квотером DEX (quote/quoter_calls.rs), который у видов «только
//! виртуальный квотер» (Metric OMM, Algebra Integral, iZiSwap, Biswap V3, SmarDex) всегда отвечал «не задана настройка».
//! 1. Metric на РЕАЛЬНОЙ записи (tests/data/metric_2026-11-15, блок 133656733, пул NVDAB/USDT 0x7201…d117): с флагом
//!    вызов пробы — по адресу виртуального квотера, quoteMetric (селектор 0x9229d10b из записи) с zeroForOne из записи
//!    для «USDT → монета»; подмена кода — код квотера по его адресу; записанный ответ (used, out) разбирается в out.
//! 2. Все пять видов: без флага — прежняя ошибка «не задана настройка dex.quoter_virtual» (и в пробе, и в квотере DEX);
//!    с флагом — проба идёт виртуальным квотером, а квотер DEX больше не советует задать уже заданную настройку.
//! 3. ЧИСТАЯ ЛОГИКА (не запись): «взял меньше входа» (сумма больше записанного used) — покупка: NoRoute (помеха, как
//!    откат QuoterV2 «не хватает ликвидности»), продажа: налог продажи не измерен (помеха); балансы продажи — числа теста.
//!    Живой записи eth_simulateV1 пробы налогов с виртуальным квотером нет — docs/TASKS.md (t174, «нужна запись»).
//! 4. Ревью 388 (Н1), ЧИСТАЯ ЛОГИКА: виртуальный квотер пробной покупки откатился (селектор отката — записанный
//!    PriceProviderFailed пула Metric QQQB), покупка прошла — помеха NoRoute, не вердикт; иначе — как раньше.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{probe_quote, quoter_calls, virtual_quoter}, check::probe_calls,
//! pools::{families, load_metric, registry}}, arb::settings (настоящая SQLite во временной папке).

use alloy_primitives::{Address, Bytes, U256, keccak256};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{LadderPoint, Pool, PoolRef, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::check::probe_calls;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::load_metric;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::probe_quote::Way;
use arb::venues::bsc_dex::quote::{quoter_calls, virtual_quoter as vq};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/metric_2026-11-15/");
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

fn json_file(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(s: &str) -> Address {
    s.parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Ответ квотера (used, out) в ABI (два слова uint256).
fn answer(used: U256, out: U256) -> Vec<u8> {
    [used.to_be_bytes::<32>(), out.to_be_bytes::<32>()].concat()
}

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-t174-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

async fn virtual_on(c: &Ctx, id: &str) {
    c.settings
        .set(
            "dex.quoter_virtual",
            Scope::Dex(id.into()),
            Some(json!(true)),
            "тест",
        )
        .await
        .unwrap();
}

/// Запись пула NVDAB/USDT Metric из probe.json / exec_sim.json (состояние — load_metric::state, как в
/// tests/bsc_dex_metric_live.rs) с видом (id, kind). Возвращает: (запись, адрес пула, порядок токенов пула token0).
fn nvdab(id: &str, kind: &str) -> (Entry, Address, Address) {
    let probe = json_file("probe.json");
    let sim = json_file("exec_sim.json");
    let nv = &probe["pools"][1];
    let pool = addr(nv["pool"].as_str().unwrap());
    let w = nv["immutables"].as_array().unwrap();
    let pool_t0 = addr(&format!("0x{:0>40}", w[1].as_str().unwrap()));
    let (usdt, coin) = (addr(USDT), pool_t0);
    let pair = if coin < usdt {
        (coin, usdt)
    } else {
        (usdt, coin)
    };
    let mut pts: Vec<LadderPoint> = sim["quoteMetric"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["pool"] == nv["pool"])
        .map(|q| LadderPoint {
            amount_in: dec(&q["amountIn"]),
            amount_out: dec(&q["out"]),
        })
        .collect();
    pts.sort_by_key(|p| p.amount_in);
    let sell: u128 = nv["prices"][0].as_str().unwrap().parse().unwrap();
    let f = families::make(id, kind).expect("вид");
    let e = Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(pool),
            token0: pair.0,
            token1: pair.1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: load_metric::state(pair, coin, pool_t0, Some(sell), pts, Vec::new()),
            block: probe["block"].as_u64().unwrap(),
            received_at: Utc::now(),
        },
        ticker: "NVDAB".into(),
        coin,
        usdt,
        key: None,
        ticks: None,
        bins: None,
        synced_block: probe["block"].as_u64().unwrap(),
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    };
    (e, pool, pool_t0)
}

/// Итог вызова симуляции (успех) с ответом.
fn ok_call(data: &[u8]) -> CallResult {
    serde_json::from_value(json!({
        "status": "0x1",
        "returnData": Bytes::copy_from_slice(data),
        "gasUsed": "0x0",
    }))
    .unwrap()
}

#[tokio::test]
async fn metric_probe_uses_virtual_quoter_on_record() {
    let c = ctx("metric").await;
    virtual_on(&c, "metric-bsc").await;
    let (e, pool, pool_t0) = nvdab("metric-bsc", "metric_omm");
    let sim = json_file("exec_sim.json");
    let rec = &sim["quoteMetric"][0];
    assert_eq!(addr(rec["pool"].as_str().unwrap()), pool);
    let amount = dec(&rec["amountIn"]);
    let way = Way::of(&c, &e);
    assert!(
        way.virt,
        "dex.quoter_virtual = да — проба виртуальным квотером"
    );
    // Подмена кода: код квотера (сверенный хеш) по его адресу.
    let over = way.overrides(&c).unwrap();
    assert_eq!(over.len(), 1);
    assert_eq!(over[0].address, vq::address());
    assert_eq!(over[0].code, vq::runtime().unwrap());
    // Покупка пробы: USDT → монета — quoteMetric с флагом из записи (в порядке токенов самого пула).
    let (to, data) = way.request(&c, &e, e.usdt, amount).unwrap();
    assert_eq!(to, vq::address());
    assert_eq!(
        &data[..4],
        &[0x92u8, 0x29, 0xd1, 0x0b][..],
        "селектор quoteMetric из записи"
    );
    let flag = rec["zeroForOne"].as_bool().unwrap();
    assert_eq!(e.usdt == pool_t0, flag);
    assert_eq!(data, vq::data_metric(pool, flag, amount));
    // Записанный ответ квотера (used = amountIn) — «ожидалось без налога» = out из записи.
    let raw = answer(dec(&rec["used"]), dec(&rec["out"]));
    assert_eq!(
        way.expected(&c, &e, &raw, e.usdt, amount).unwrap(),
        Some(dec(&rec["out"]))
    );
    assert_eq!(
        way.expected(&c, &e, &[1, 2, 3], e.usdt, amount).unwrap(),
        None
    );
    // Продажа: тот же разбор в sell_result (балансы — числа теста, чистая логика).
    let (b0, b1) = (U256::from(5u8), U256::from(1005u64));
    let calls = [
        ok_call(&raw),
        ok_call(&b0.to_be_bytes::<32>()),
        ok_call(&[]),
        ok_call(&b1.to_be_bytes::<32>()),
    ];
    assert_eq!(
        probe_calls::sell_result(&c, &e, way, &calls, (true, amount)),
        Ok((dec(&rec["out"]), b1 - b0))
    );
}

/// ЧИСТАЯ ЛОГИКА на записанном ответе: сумма на 1 больше записанного used — «взял меньше входа».
#[tokio::test]
async fn short_answer_is_interference() {
    let c = ctx("short").await;
    virtual_on(&c, "metric-bsc").await;
    let (e, _, _) = nvdab("metric-bsc", "metric_omm");
    let sim = json_file("exec_sim.json");
    let rec = &sim["quoteMetric"][0];
    let raw = answer(dec(&rec["used"]), dec(&rec["out"]));
    let more = dec(&rec["used"]) + U256::from(1u8);
    let way = Way::of(&c, &e);
    let err = way.expected(&c, &e, &raw, e.usdt, more).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoRoute);
    assert!(err.message.contains(vq::SHORT), "{}", err.message);
    let calls = [ok_call(&raw), ok_call(&[]), ok_call(&[]), ok_call(&[])];
    let miss = probe_calls::sell_result(&c, &e, way, &calls, (true, more)).unwrap_err();
    assert!(!miss.reverted && miss.why.contains(vq::SHORT), "{miss:?}");
}

/// Все виды «только виртуальный квотер»: без флага — прежнее «не задана настройка», с флагом — проба виртуальным
/// квотером, текст квотера DEX без совета задать заданное.
#[tokio::test]
async fn only_virtual_kinds_with_and_without_flag() {
    let kinds = [
        ("metric-bsc", "metric_omm"),
        ("integral-bsc", "algebra_integral"),
        ("izi-bsc", "iziswap"),
        ("biswap-v3-bsc", "biswap_v3"),
        ("smardex-bsc", "smardex"),
    ];
    let c = ctx("kinds").await;
    let amount = U256::from(10u8).pow(U256::from(18u8));
    for (id, kind) in kinds {
        let (e, pool, _) = nvdab(id, kind);
        // Без флага — как раньше: NotConfigured с именем настройки (и в квотере DEX, и в пробе).
        let off = Way::of(&c, &e);
        assert!(!off.virt, "{kind}");
        assert!(off.overrides(&c).unwrap().is_empty());
        for err in [
            quoter_calls::request(&c, &e, e.usdt, amount).unwrap_err(),
            off.request(&c, &e, e.usdt, amount).unwrap_err(),
        ] {
            assert_eq!(
                err.code,
                ErrorCode::NotConfigured,
                "{kind}: {}",
                err.message
            );
            assert_eq!(err.setting.as_deref(), Some("dex.quoter_virtual"), "{kind}");
            assert!(err.message.contains("задайте dex.quoter_virtual"), "{kind}");
        }
        virtual_on(&c, id).await;
        let on = Way::of(&c, &e);
        assert!(on.virt, "{kind}");
        let (to, data) = on.request(&c, &e, e.usdt, amount).unwrap();
        assert_eq!(to, vq::address(), "{kind}");
        let zfo = e.usdt == e.pool.token0;
        let want = match kind {
            "metric_omm" => data.clone(),
            "iziswap" | "biswap_v3" => vq::data_izi(pool, zfo, amount),
            "algebra_integral" => vq::data(pool, zfo, amount),
            _ => {
                let sel = keccak256(b"quoteSmardex(address,bool,uint256)");
                assert_eq!(&data[..4], &sel[..4], "{kind}");
                data.clone()
            }
        };
        assert_eq!(data, want, "{kind}");
        // Квотер DEX при заданном флаге: не «не задана настройка», а ошибка кода без совета задать заданное.
        let err = quoter_calls::request(&c, &e, e.usdt, amount).unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal, "{kind}: {}", err.message);
        assert!(!err.message.contains("задайте"), "{kind}: {}", err.message);
    }
}

/// Итог отката симуляции с данными отката.
fn reverted_call(data: &[u8]) -> CallResult {
    serde_json::from_value(json!({
        "status": "0x0",
        "returnData": Bytes::copy_from_slice(data),
        "gasUsed": "0x0",
        "error": { "code": 3, "message": "execution reverted" },
    }))
    .unwrap()
}

/// Ревью 388 (Н1), ЧИСТАЯ ЛОГИКА: селектор отката — записанный откат пула Metric QQQB PriceProviderFailed
/// (probe.json, prices_revert; оракул пула), сочетание «квотер откатился, покупка прошла» — условное (симуляции пробы с
/// таким откатом не записано). Виртуальный квотер откатился на своём шаге, покупка прошла — помеха (NoRoute), не
/// вердикт; покупка тоже откатилась, откат не на шаге квотера или квотер не виртуальный — решать как раньше (None).
#[tokio::test]
async fn virtual_quote_revert_with_buy_ok_is_interference() {
    let c = ctx("n1").await;
    let (e, _, _) = nvdab("metric-bsc", "metric_omm");
    let probe = json_file("probe.json");
    let rec = probe["pools"][3]["prices_revert"].as_str().unwrap();
    let sel = alloy_primitives::hex::decode(&rec[2..10]).unwrap();
    let r = reverted_call(&sel);
    let buy_ok = ok_call(&[]);
    let buy_bad = reverted_call(&[]);
    let (virt, plain) = (Way { virt: true }, Way { virt: false });
    let (q, x) = (3usize, 6usize);
    let err = virt
        .quote_reverted(&c, &e, (q, q), &r, Some(&buy_ok))
        .expect("помеха");
    assert_eq!(err.code, ErrorCode::NoRoute);
    assert!(err.message.contains("покупка прошла"), "{}", err.message);
    assert!(err.message.contains(&r.why()), "{}", err.message);
    assert!(
        virt.quote_reverted(&c, &e, (q, q), &r, Some(&buy_bad))
            .is_none()
    );
    assert!(virt.quote_reverted(&c, &e, (q, q), &r, None).is_none());
    assert!(
        virt.quote_reverted(&c, &e, (x, q), &r, Some(&buy_ok))
            .is_none()
    );
    assert!(
        plain
            .quote_reverted(&c, &e, (q, q), &r, Some(&buy_ok))
            .is_none()
    );
}
