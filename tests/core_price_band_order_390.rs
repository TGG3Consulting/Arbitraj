//! Файл: tests/core_price_band_order_390.rs
//! Что проверяет: решение 390 (ревью 2 t180, Б1) на настоящем ядре с SQLite во временной папке (реестр шлюза пуст,
//! виды площадок — в inner.kinds, как tests/core_precheck_fast_266.rs; моков нет):
//! - чтение коридора (core/price_band.rs read) не собирает контекст направления и не зависит от справочника сетей:
//!   у биржи без пары в справочнике пар — строка «коридор цены … не прочитан: …», у DEX — ничего, расчёт без коридора;
//! - контекст расчёта с коридорами (context) собирается отдельно, ПОСЛЕ шагов сторон: без прочитанного коридора — нет
//!   контекста (пересчёт соберёт его сам, как до решения 390); коридор прочитан, а контекст не собирается — строка
//!   «коридор цены не учтён: …», контекста нет.
//!
//! Что план предпроверки берёт справочник сетей, обновлённый шагом стороны (transfer_open → networks_merge), здесь
//! не проверяется: шаг стороны с запросом к бирже требует подключённой площадки, а площадка-заглушка запрещена
//! (CLAUDE.md); порядок обеспечен кодом precheck.rs (context вызывается после try_join шагов сторон) — docs/TASKS.md.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::price_band, tests/report_common.

mod report_common;

use arb::core::price_band::{Bands, context, read};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::price_band::PriceBand;
use report_common::{dir, start};

fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

#[tokio::test]
async fn read_needs_no_context_and_context_is_separate() {
    let (core, st) = start(&dir("band-order-390")).await;
    let kinds = [("bingx", VenueKind::Cex), ("bsc_dex", VenueKind::Dex)];
    *core.inner.kinds.write().unwrap() = kinds.iter().map(|(id, k)| (v(id), *k)).collect();
    st.venue_kinds()
        .set(kinds.iter().map(|(id, k)| (id.to_string(), *k)));
    let d = Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: v("bingx"),
        sell_on: v("bsc_dex"),
    };
    // Справочника пар нет: у биржи — строка «не прочитан», у DEX — ничего; коридоров нет.
    let mut bands = Box::pin(read(&core, &d)).await;
    assert!(!bands.any());
    assert_eq!(bands.lines.len(), 1, "{:?}", bands.lines);
    assert!(
        bands.lines[0].starts_with("коридор цены bingx не прочитан: "),
        "{:?}",
        bands.lines
    );
    // Без коридора контекст не собирается (пересчёт соберёт его сам), строк не добавляется.
    assert!(context(&core, &d, &mut bands).is_none());
    assert_eq!(bands.lines.len(), 1);
    // Коридор прочитан, контекст не собирается (монеты нет в справочнике ядра) — честная строка, контекста нет.
    let mut got = Bands {
        a: Some(Box::new(PriceBand {
            buy_max: Some("1.2".parse().unwrap()),
            note: "тест".into(),
            ..PriceBand::default()
        })),
        ..Bands::default()
    };
    assert!(context(&core, &d, &mut got).is_none());
    assert!(
        got.lines
            .iter()
            .any(|l| l.starts_with("коридор цены не учтён: контекст направления не собран")),
        "{:?}",
        got.lines
    );
}
