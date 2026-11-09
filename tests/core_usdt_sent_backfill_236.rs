//! Файл: tests/core_usdt_sent_backfill_236.rs
//! Что делает: решение 236, дополнение п. 5 (техлид 09.11) — разовая запись «ушло» в старые части возврата USDT по
//! журналу core.transfer_sent (core/usdt_sent_backfill.rs, программа src/bin/usdt_sent_backfill.rs). Настоящая SQLite во
//! временной папке, живое ядро на ней после записи (моков нет). Данные — живая сделка 20261108205335162HELLO (mexc → DEX:
//! запрошено 27.655454087068799999999999694, ушло 27.6 — точность вывода USDT у MEXC 0.1, зачислено 27.59;
//! docs/notes/deal-accounting-2026-11-09.md). Текст записи журнала — формат core/transfers.rs («<id> <ушло> <монета>:
//! <откуда> → <куда>, хеш <tx>»; хеш в разборе не сохранился — здесь нулевой, он не разбирается).
//! Проверяется: (1) разбор записи и хвоста id перегона (u, u<n>, m<n>; другие — нет); (2) план: найдено — исправление,
//! нет записи / две записи / ушло вне [зачислено, запрошено] — пропуск с причиной; фильтр по площадке; (3) запись —
//! только поле sent нужной части, остальной JSON как был; повторный план — пусто; (4) ядро после записи: затраты
//! возврата 0.01 (ушло − зачислено), а не 0.0655 (запрошено − зачислено).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::storage, tests/usdt_return_common.

mod usdt_return_common;

use std::str::FromStr;

use arb::core::deal::Step;
use arb::core::types::VenueId;
use arb::core::usdt_return_state::UsdtReturn;
use arb::core::usdt_sent_backfill::{self as bf, Sent};
use arb::storage::db::Db;
use arb::storage::journal::Level;
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, start};

/// Число из строки (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Площадка.
fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

const HELLO: &str = "20261108205335162HELLO";
const AMOUNT: &str = "27.655454087068799999999999694";

/// Текст записи журнала core.transfer_sent (формат core/transfers.rs).
fn line(id: &str, qty: &str, from: &str, to: &str) -> String {
    format!(
        "{id} {qty} USDT: {from} → {to}, хеш 0x0000000000000000000000000000000000000000000000000000000000000000"
    )
}

#[test]
fn parse_and_id_tail() {
    let s = bf::parse(&line(&format!("{HELLO}u"), "27.6", "mexc", "bsc_dex"), "t").unwrap();
    assert_eq!(
        s,
        Sent {
            id: format!("{HELLO}u"),
            qty: d("27.6"),
            ticker: "USDT".into(),
            from: "mexc".into(),
            to: "bsc_dex".into(),
            at: "t".into(),
        }
    );
    assert!(bf::parse("Сделка x: что-то другое", "t").is_none());
    for (id, ok) in [
        ("u", true),
        ("u3", true),
        ("m123456", true),
        ("", false),
        ("m", false),
        ("x", false),
        ("u3a", false),
    ] {
        assert_eq!(bf::is_return_of(&format!("{HELLO}{id}"), HELLO), ok, "{id}");
    }
    // Перевод монеты сделки — id без хвоста: не перегон USDT.
    assert!(!bf::is_return_of(HELLO, HELLO));
}

#[tokio::test]
async fn backfill_writes_only_sent_and_core_uses_it() {
    let p = dir("a236bf");
    {
        let (core, j) = start(&p, None).await;
        // Старая запись: часть «вернулись» без sent (до решения 236).
        let mut x = doge(HELLO);
        x.step = Step::Closed;
        x.result_usd = Some(d("1"));
        x.usdt_return = Some(UsdtReturn::Done {
            amount: d(AMOUNT),
            credited: d("27.59"),
            from: v("mexc"),
            to: v("bsc_dex"),
            sent: None,
            fee_usd: None,
        });
        arb::core::ledger::save(&core, &x).await;
        // Вторая сделка — две записи одного перегона (неясно), третья — записи нет.
        for id in ["b2DOGE", "b3DOGE"] {
            let mut y = x.clone();
            y.id = id.into();
            arb::core::ledger::save(&core, &y).await;
        }
        for (id, qty) in [
            (format!("{HELLO}u"), "27.6"),
            ("b2DOGEu".into(), "27.6"),
            ("b2DOGEu1".into(), "27.6"),
        ] {
            j.write(
                Level::Info,
                bf::SENT_KIND,
                Some("mexc"),
                line(&id, qty, "mexc", "bsc_dex"),
                None,
            )
            .await
            .unwrap();
        }
        // Перевод монеты (не перегон USDT) той же сделки — не сопоставляется.
        j.write(
            Level::Info,
            bf::SENT_KIND,
            Some("mexc"),
            line(HELLO, "100", "mexc", "bsc_dex"),
            None,
        )
        .await
        .unwrap();
    }
    let db = Db::open(&p.join("t.sqlite")).unwrap();
    let (mut deals, sents) = bf::load(&db).await.unwrap();
    let (fixes, skips) = bf::plan(&deals, &sents, &[]);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    let f = &fixes[0];
    assert_eq!((f.deal.as_str(), f.slot, f.sent), (HELLO, 0, d("27.6")));
    let why = |id: &str| {
        skips
            .iter()
            .find(|s| s.deal == id)
            .map(|s| s.why.clone())
            .unwrap()
    };
    assert!(why("b2DOGE").contains("2 записей"), "{}", why("b2DOGE"));
    assert!(why("b3DOGE").contains("нет записи"), "{}", why("b3DOGE"));
    // Фильтр по площадке: только bingx — ничего.
    assert_eq!(
        bf::plan(&deals, &sents, &["bingx".into()]),
        (vec![], vec![])
    );
    // Ушло больше запрошенного — пропуск.
    let bad = vec![Sent {
        qty: d("28"),
        ..sents[0].clone()
    }];
    let (none, sk) = bf::plan(&deals[..], &bad, &[]);
    assert!(none.is_empty() && sk.iter().any(|s| s.why.contains("больше запрошенного")));
    // Запись: только поле sent у этой части.
    let (id, body) = deals.iter_mut().find(|(id, _)| id == HELLO).unwrap();
    let before = body.clone();
    assert!(bf::apply(body, f));
    let mut want = before.clone();
    want["usdt_return"]["sent"] = serde_json::Value::String("27.6".into());
    assert_eq!(*body, want);
    assert!(!bf::apply(body, f), "второй раз не пишется");
    assert_eq!(
        bf::write(&db, vec![(id.clone(), body.to_string())])
            .await
            .unwrap(),
        1
    );
    let (deals2, sents2) = bf::load(&db).await.unwrap();
    assert!(
        bf::plan(&deals2, &sents2, &[]).0.is_empty(),
        "повторный запуск — нечего писать"
    );
    // Живое ядро на этой базе: затраты возврата — ушло − зачислено (0.01), итог 1 − 0.01.
    let (core, _) = start(&p, None).await;
    let y = core.deals().into_iter().find(|x| x.id == HELLO).unwrap();
    assert_eq!(y.return_cost_usd, Some(d("0.01")));
    assert_eq!(y.result_usd, Some(d("0.99")));
    let z = core.deals().into_iter().find(|x| x.id == "b3DOGE").unwrap();
    assert_eq!(
        z.return_cost_usd,
        Some(d(AMOUNT) - d("27.59")),
        "без факта — как раньше"
    );
}
