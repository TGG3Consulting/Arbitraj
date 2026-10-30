//! Файл: tests/bsc_dex_elastic_family.rs
//! Что делает: подключение семейства KyberSwap Elastic (решение 81) без сети — таблица сопоставления (dex.kind =
//! kyber_elastic), набор обязательных настроек (фабрика/роутер/квотер/читатель тиков, БЕЗ Universal Router и Permit2 —
//! форк), и заглушка сверки котировки с квотером на реальной записи (пока записи нет — #[ignore]).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::families, arb::settings (настоящая SQLite во временной папке).
//! Математика Elastic на официальных значениях — tests/bsc_dex_elastic_math.rs (81a).

use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::families::{self, Proto};

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-elastic-{tag}-{}", std::process::id()));
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
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

async fn set(c: &Ctx, list: &[(&str, Scope, Value)]) {
    for (n, s, v) in list {
        c.settings
            .set(n, s.clone(), Some(v.clone()), "тест")
            .await
            .unwrap();
    }
}

/// kyber_elastic: в графе, исполним своим роутером (graphable+tradable+fork), требует фабрику/роутер/квотер/читатель
/// тиков и НЕ требует Universal Router/Permit2 (форк, решения 59, 81, 82).
#[tokio::test]
async fn elastic_family_requirements() {
    let c = ctx("req").await;
    let id = "kyberswap-elastic-bsc";
    let s = Scope::Dex(id.into());
    let a = json!("0x1111111111111111111111111111111111111111");

    // Один вид — ещё не подключён (нет остальных настроек).
    set(&c, &[("dex.kind", s.clone(), json!("kyber_elastic"))]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("не задана dex.pool_enabled"),
        "{}",
        families::classify(&c, id).unwrap_err()
    );

    let f = families::make(id, "kyber_elastic").unwrap();
    assert_eq!(f.proto, Proto::KyberElastic);
    assert_eq!((f.graphable, f.tradable, f.fork), (true, true, true));

    // Обязательные настройки: фабрика, роутер, квотер, читатель тиков — и ни UR, ни Permit2.
    let req = families::required(&c, f);
    for name in ["dex.factory", "dex.router", "dex.quoter", "dex.tick_reader"] {
        assert!(req.contains(&name), "нет {name} в required(): {req:?}");
    }
    for name in [
        "dex.universal_router",
        "dex.universal_router_version",
        "dex.permit2",
    ] {
        assert!(
            !req.contains(&name),
            "{name} не нужен форку Elastic: {req:?}"
        );
    }

    // Заданы все обязательные — подключена.
    set(
        &c,
        &[
            ("dex.pool_enabled", s.clone(), json!(true)),
            ("dex.factory", s.clone(), a.clone()),
            ("dex.router", s.clone(), a.clone()),
            ("dex.quoter", s.clone(), a.clone()),
            ("dex.tick_reader", s.clone(), a.clone()),
        ],
    )
    .await;
    let f = families::classify(&c, id).unwrap();
    assert_eq!((f.graphable, f.tradable, f.fork), (true, true, true));
}

/// Сверка локальной котировки Elastic (math/swap_elastic, 81a) с ответом QuoterV2 на РЕАЛЬНОЙ записи пула
/// (record_elastic.js). Пока записи нет — тест отключён (#[ignore]); включить, положив запись в
/// tests/data/elastic_<дата>/responses.json и сняв #[ignore]. Решение 65: расхождение не больше
/// dex.quoter_max_diff_pct.
#[test]
#[ignore = "нужна запись record_elastic.js (ещё не снята в браузере) — tests/data/elastic_*/responses.json"]
fn elastic_quote_matches_quoter() {
    // TODO(81b): прочитать запись (getPoolState/getLiquidityState/swapFeeUnits, список ticks + ticks(t),
    // quote N), собрать ElasticState + TickMap, прогнать math::swap_elastic::swap по каждому probe и сверить
    // amount_out с QuoterV2.quoteExactInputSingle.returnedAmount в пределах dex.quoter_max_diff_pct.
    panic!("запись record_elastic.js ещё не снята — см. tests/recorders/record_elastic.js");
}
