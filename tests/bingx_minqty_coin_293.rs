//! Файл: tests/bingx_minqty_coin_293.rs
//! Что делает: ревью решения 293 — venue.ignore_deprecated_qty у монеты и площадки на записанных ответах BingX 15.11.2026
//! (tests/data/bingx_minqty_2026-11-15, все 9 пар одним справочником). Ключ монеты — тот, что пишет интерфейс и
//! миграция решения 211: coin_id по адресу контракта BSC (адрес SHOPINX — tests/data/shopinx_trades_2026-11-15).
//! 1) «да» только у SHOPINX — пределы сняты только у неё; «да» у площадки и «нет» у SHOPINX — у SHOPINX остались,
//!    у остальных сняты; монета без адреса — значение монеты не применяется, действует площадка;
//! 2) старое значение по тикеру («coin:SHOPINX») адаптер не читает; после переноса миграцией решения 211
//!    (core/coin_scope_migrate.rs plan + apply) — читается;
//! 3) «да → нет» — пределы вернулись; смена видна в журнале (pairs.rs changes, only_qty_changed, qty_changed);
//! 4) значение неверного вида (повреждение базы) у монеты — как «не задано» (действует площадка), справочник
//!    обновляется, Warn в журнал venue.symbols один раз.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/bingx_293_common, arb::venues::bingx (symbols_qty, pairs), arb::core::coin_scope_migrate.

mod bingx_293_common;

use std::collections::{BTreeMap, BTreeSet};

use arb::core::coin_scope_migrate::{apply, old_ids, plan};
use arb::settings::value::Scope;
use arb::storage::journal::Level;
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::symbols_qty;
use bingx_293_common::{
    bare, ctx, ctx_raw, d, has_qty, load, load_all, set, set_coin, shopinx, shopinx_contract,
};
use serde_json::json;

/// Остальные 8 пар выборки (монеты без адреса: к ним применяется только значение площадки).
const OTHERS: [&str; 8] = [
    "CAKE",
    "BNB",
    "TWT",
    "SFP",
    "BTC",
    "DOGE",
    "MUBARAK",
    "BANANAS31",
];

#[tokio::test]
async fn coin_value_beats_venue_value() {
    let c = ctx("coin").await;
    let a = shopinx();
    // «Да» только у SHOPINX (ключ — адрес) — пределы сняты только у неё.
    set_coin(&c.settings, &a, true).await;
    let s = load_all(&c);
    assert!(!has_qty(&s, &c, &a));
    assert!(OTHERS.iter().all(|t| has_qty(&s, &c, &bare(t))));
    // Та же монета без адреса — значение монеты не применяется (площадка не задана — пределы есть).
    assert!(has_qty(&s, &c, &bare("SHOPINX")));
    // «Да» у площадки и «нет» у SHOPINX — у SHOPINX пределы остались, у остальных сняты.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    set_coin(&c.settings, &a, false).await;
    assert!(has_qty(&s, &c, &a));
    assert!(OTHERS.iter().all(|t| !has_qty(&s, &c, &bare(t))));
    // Без адреса — значение площадки («да»).
    assert!(!has_qty(&s, &c, &bare("SHOPINX")));
}

#[tokio::test]
async fn ticker_key_read_only_after_migration() {
    let c = ctx("migr").await;
    let a = shopinx();
    // Старый ключ по тикеру — так писали настройки монеты до решения 211.
    let old = Scope::Coin("SHOPINX".into());
    c.settings
        .set(symbols_qty::SETTING, old.clone(), Some(json!(true)), "тест")
        .await
        .unwrap();
    let s = load(&c);
    assert!(
        has_qty(&s, &c, &a),
        "значение по тикеру без переноса не читается"
    );
    // Перенос решения 211: у тикера один адрес — «coin:SHOPINX» → «coin:0x…».
    let by: BTreeMap<String, BTreeSet<_>> =
        [("SHOPINX".to_string(), [shopinx_contract()].into())].into();
    let p = plan(&old_ids(&c.settings), &by);
    let (moved, conflicts, failed) = apply(&c.settings, &p).await;
    assert_eq!((moved, conflicts.len(), failed.len()), (1, 0, 0));
    assert!(c.settings.get_exact(symbols_qty::SETTING, &old).is_none());
    assert!(
        !has_qty(&s, &c, &a),
        "после переноса значение монеты читается"
    );
}

#[tokio::test]
async fn yes_then_no_returns_limits_and_journal_text() {
    let c = ctx("back").await;
    let a = shopinx();
    let assets = [a.clone()];
    let s = load(&c);
    let before = Snapshot::build(&s, &assets);
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let on = Snapshot::build(&s, &assets);
    // Смена видна в журнале venue.symbols и отличается только пределами количества.
    assert!(on.only_qty_changed(&before) && on.qty_changed(&before));
    assert_eq!(
        on.changes(&before),
        vec![
            "SHOPINX: API: покупка да, продажа да → API: покупка да, продажа да, пределов количества нет"
        ]
    );
    // «Да → нет» — пределы вернулись.
    set(&c.settings, symbols_qty::SETTING, Some(json!(false))).await;
    let r = s.get_for(&c, &a).unwrap();
    assert_eq!(
        (r.min_qty, r.max_qty),
        (Some(d("57505")), Some(d("575043128.24")))
    );
    let off = Snapshot::build(&s, &assets);
    assert_eq!(off, before);
    assert!(!off.only_qty_changed(&before) && !off.qty_changed(&before));
}

#[tokio::test]
async fn bad_coin_value_is_not_set_and_warned_once() {
    let a = shopinx();
    let coin = format!("coin:{:#x}", shopinx_contract());
    // Значение монеты — строка вместо да/нет (повреждение базы); у площадки — «да».
    let rows = [
        (symbols_qty::SETTING, coin.as_str(), "\"да\""),
        (symbols_qty::SETTING, "venue:bingx", "true"),
    ];
    let c = ctx_raw("bad", &rows).await;
    // Справочник ставится; у монеты действует значение площадки («да»).
    let s = load(&c);
    assert!(!has_qty(&s, &c, &a));
    // Warn — один раз за процесс на область.
    assert_eq!(symbols_qty::warn_bad(&c).await, vec![coin.clone()]);
    assert!(symbols_qty::warn_bad(&c).await.is_empty());
    let log = c.journal.list(500, None).await.unwrap();
    let warned = log
        .iter()
        .filter(|j| {
            j.kind == "venue.symbols" && j.level == Level::Warn && j.message.contains(&coin)
        })
        .count();
    assert_eq!(warned, 1);
}
