//! Файл: tests/core_fit_volume_294.rs
//! Что проверяет: решение 294 (прямой способ) — путь через промежуточную монету, которому не хватает прочитанной карты
//! тиков, при dex.intermediate_partial_fill = да отдаёт помещающуюся часть (fillable_qty < qty) одним расчётом по
//! пулу (quote/route_fit.rs, вызов — quote/via_pools.rs), на РЕАЛЬНОЙ записи цепи 15.11.2026
//! (tests/data/fit_volume_2026-11-15, блок 133732042): пул PancakeSwap V3 SHOPINX/WBNB (fee 100, tickSpacing 1) и пара
//! PancakeSwap V2 WBNB/USDT. Пула SHOPINX с USDT нет — котировка идёт только через WBNB.
//! Записано с цепи: состояние пулов и слова битмапа ±12. Задано разработчиком (README записи): карта ±2 слова
//! (dex.tick_words = 2 — значение на сервере 15.11), объём 57000 SHOPINX (верх живого диапазона 33–57 тыс.), цена газа
//! 1 gwei, нулевые налоги SHOPINX и WBNB, допуск пулов. Котировка — выбор маршрута адаптера целиком (quote/routing.rs
//! run, как Quoting::priced до газа разрешений и чаевых; цену газа узла тест не спрашивает).
//! Время котировки (поле at) при сравнении «1 в 1» не учитывается.
//! Проверяется: (1) настройка не задана / «нет» — прежний NoRoute, тот же текст; (2) да — fillable = помещающийся
//! объём: сверка тем же кодом без настройки — на нём котировка полная, на 1 наименьшую единицу SHOPINX больше —
//! прежний NoRoute; продажа (предел на первом шаге) и покупка (предел на последнем шаге); (3) объём помещается —
//! котировка 1 в 1 с настройкой и без; (4) V2 «выход ≥ резерва» (записанная пара WBNB/USDT) — помещающейся части нет (прежний NoRoute).
//! Нет записи: предел на втором шаге продажи / первом шаге покупки (нужен путь с узким вторым пулом), налог монеты ≠ 0
//! — docs/TASKS.md.
//! Кто вызывает: cargo test.
//! Режим graph — tests/bsc_dex_fit_graph_294.rs.
//! От чего зависит: arb::venues::bsc_dex::quote::{routing, route_fit, route_calc}, tests/fit_common (сцена, котировка),
//! tests/route_common.

mod fit_common;
mod route_common;

use rust_decimal::Decimal;

use arb::core::types::Side;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::route_fit;
use arb::venues::bsc_dex::quote::select::Usable;
use fit_common::{USDT, WBNB, d, full, js, partial, scene, scene_with_words};

/// На том же блоке граница узкой карты не должна ограничивать большой ордер на широкой
/// карте или увеличивать малый ордер. Все слова ±12 действительно записаны в TickLens.
#[tokio::test]
async fn recorded_word_coverage_limits_the_quote_not_the_requested_order() {
    let narrow = scene().await;
    let wide = scene_with_words(12).await;
    partial(&narrow, Some(true)).await;
    partial(&wide, Some(true)).await;
    let narrow = narrow.quoting();
    let wide = wide.quoting();

    let requested = d("57000");
    for (side, narrow_limit) in [
        (Side::Sell, d("29666.114904110584943289")),
        (Side::Buy, d("28380.895110992572453266")),
    ] {
        let clipped = full(&narrow, side, requested).await.unwrap();
        let complete = full(&wide, side, requested).await.unwrap();
        assert_eq!(clipped.fillable_qty, narrow_limit, "{side:?}");
        assert_eq!(complete.fillable_qty, requested, "{side:?}");

        let small = d("1000");
        let old = full(&narrow, side, small).await.unwrap();
        let new = full(&wide, side, small).await.unwrap();
        assert_eq!(new.fillable_qty, small, "{side:?}");
        assert_eq!(new.avg_price, old.avg_price, "{side:?}");
    }

    // Для CEX → DEX при h = 0.7 ядро дополнительно требует глубину sell_qty / h.
    // 81 429 — целый объём чуть выше 57 000 / 0.7, только запрос глубины.
    let depth_probe = d("81429");
    let depth = full(&wide, Side::Sell, depth_probe).await.unwrap();
    assert_eq!(depth.fillable_qty, depth_probe);
}

#[tokio::test]
async fn partial_fill_gives_exact_fitting_volume() {
    let p = scene().await;
    let q = p.quoting();
    let want = d("57000");
    let unit = Decimal::new(1, 18);
    for side in [Side::Sell, Side::Buy] {
        // (1) не задана — прежний NoRoute; «нет» — тот же текст.
        partial(&p, None).await;
        let off = full(&q, side, want).await.unwrap_err();
        assert_eq!(off.code, ErrorCode::NoRoute);
        assert!(
            off.message.contains("не исполним целиком"),
            "{}",
            off.message
        );
        partial(&p, Some(false)).await;
        let no = full(&q, side, want).await.unwrap_err();
        assert_eq!((no.code, &no.message), (off.code, &off.message));
        // (2) да — помещающаяся часть.
        partial(&p, Some(true)).await;
        let got = full(&q, side, want).await.unwrap();
        let fit = got.fillable_qty;
        assert!(fit > Decimal::ZERO && fit < want, "{side:?}: {fit}");
        println!(
            "{side:?}: 57000 не помещается — помещается {fit}, средняя {}",
            got.avg_price
        );
        // Сверка тем же кодом без настройки: на помещающемся объёме полная, на единицу больше — прежний NoRoute.
        partial(&p, None).await;
        let at = full(&q, side, fit).await.unwrap();
        assert_eq!(at.fillable_qty, fit);
        // Часть = полная котировка на этом объёме; отличается только запрошенный объём (qty — 57000).
        assert_eq!(got.qty, want);
        let mut part = got.clone();
        part.qty = fit;
        assert_eq!(
            js(&Ok(at)),
            js(&Ok(part)),
            "{side:?}: часть = полная котировка на ней"
        );
        let over = full(&q, side, fit + unit).await.unwrap_err();
        assert_eq!(over.code, ErrorCode::NoRoute, "{}", over.message);
        // (3) помещается — 1 в 1 с настройкой и без.
        let small = d("1000");
        let a = full(&q, side, small).await;
        partial(&p, Some(true)).await;
        let b = full(&q, side, small).await;
        assert_eq!(js(&a), js(&b));
        assert_eq!(a.unwrap().fillable_qty, small);
    }
}

/// V2, покупка, выход ≥ резерва (math/v2.rs amount_in): частичного итога у V2 нет — помещающейся части нет, прежний
/// NoRoute (резерв − 1 помещается только при бесконечной цене — DECISIONS 294). Записанная пара WBNB/USDT: покупка
/// WBNB за USDT на весь резерв WBNB.
#[tokio::test]
async fn v2_reserve_limit_keeps_no_route() {
    let p = scene().await;
    let pair = p.registry.by_pair(WBNB, USDT).into_iter().next().unwrap();
    let reserve_wbnb = match pair.pool.state {
        arb::core::types::PoolState::V2 { reserve1, .. } => reserve1,
        _ => panic!("ожидалась пара V2"),
    };
    let qty = arb::venues::bsc_dex::num::to_decimal(reserve_wbnb, 18).unwrap();
    let usable = Usable {
        gas: 120_000,
        hook_pct: Decimal::ZERO,
        tax_pct: Decimal::ZERO,
        hook: Default::default(),
    };
    let legs = vec![Leg {
        entry: pair,
        usable,
        token_in: USDT,
        token_out: WBNB,
        dec_in: 18,
        dec_out: 18,
    }];
    let e = route_calc::eval(&p.ctx, &legs, Side::Buy, qty, Decimal::ONE).unwrap_err();
    assert_eq!(e.code, ErrorCode::NoRoute, "{}", e.message);
    assert!(route_fit::fit(&p.ctx, &legs, Side::Buy, qty, Decimal::ONE).is_none());
}
