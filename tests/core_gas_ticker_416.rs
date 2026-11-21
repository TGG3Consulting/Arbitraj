//! Файл: tests/core_gas_ticker_416.rs
//! Что делает: решение 416 (ревью шага 1) — тикер газа сети из настройки площадки DEX dex.gas_ticker вместо «BNB» в
//! коде: (1) ядро (core/gas_ticker.rs) — не задана: точная причина «Не задана настройка dex.gas_ticker»; задана — её
//! значение; две площадки DEX с разными тикерами — без площадки газ не выбрать, по площадке — её; площадка-биржа —
//! тикер сети площадок DEX; (2) адаптер (venues/bsc_dex/gas_ticker.rs) — read из настроек; sent (затраты после
//! отправки, решение 398) — тикер, прочитанный перед отправкой, даже если настройку после неё убрали.
//! Ядро — t198_common (SQLite во временной папке; bingx, mexc — биржи, bsc_dex — DEX); окружение DEX — route_common.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::gas_ticker, arb::venues::bsc_dex::gas_ticker, arb::settings. Моков нет.

mod route_common;
mod t198_common;

use arb::core::gas_ticker;
use arb::core::types::VenueId;
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;
use serde_json::json;

/// Задаёт (или убирает) dex.gas_ticker площадке.
async fn set(s: &arb::settings::store::Store, venue: &str, v: Option<&str>) {
    s.set(
        "dex.gas_ticker",
        Scope::Venue(venue.into()),
        v.map(|x| json!(x)),
        "тест",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn core_gas_ticker_from_dex_setting() {
    let core = t198_common::start("gas-ticker-416").await;
    let s = core.inner.deps.settings.clone();
    let dex = VenueId("bsc_dex".into());
    let e = gas_ticker::at(&s, &dex).unwrap_err();
    assert!(e.contains("dex.gas_ticker"), "{e}");
    assert!(gas_ticker::of_core(&core).is_err());
    set(&s, "bsc_dex", Some("BNB")).await;
    assert_eq!(gas_ticker::at(&s, &dex).unwrap(), "BNB");
    assert_eq!(gas_ticker::of_core(&core).unwrap(), "BNB");
    assert_eq!(
        gas_ticker::for_venue(&core, &VenueId("mexc".into())).unwrap(),
        "BNB",
        "биржа — газ сети площадок DEX"
    );
    // Вторая площадка DEX другой сети: без площадки — ошибка, по площадке — её тикер.
    core.inner
        .kinds
        .write()
        .unwrap()
        .push((VenueId("arb_dex".into()), VenueKind::Dex));
    set(&s, "arb_dex", Some("ETH")).await;
    let e = gas_ticker::of_core(&core).unwrap_err();
    assert!(e.contains("разные тикеры газа"), "{e}");
    assert_eq!(
        gas_ticker::for_venue(&core, &VenueId("arb_dex".into())).unwrap(),
        "ETH"
    );
}

#[tokio::test]
async fn dex_gas_ticker_read_and_sent() {
    let ctx = route_common::ctx().await;
    use arb::venues::bsc_dex::gas_ticker::{read, sent, ticker};
    assert_eq!(read(&ctx).unwrap(), "BNB");
    assert_eq!(sent(&ctx).unwrap(), "BNB", "в памяти нет — из настроек");
    set(&ctx.settings, "bsc_dex", Some("ETH")).await;
    assert_eq!(ticker(&ctx).unwrap(), "ETH", "перед отправкой — в память");
    set(&ctx.settings, "bsc_dex", None).await;
    let e = read(&ctx).unwrap_err();
    assert!(e.message.contains("dex.gas_ticker"), "{}", e.message);
    assert_eq!(
        sent(&ctx).unwrap(),
        "ETH",
        "после отправки — прочитанный перед ней (решение 398)"
    );
}
