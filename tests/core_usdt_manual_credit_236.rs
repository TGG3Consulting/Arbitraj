//! Файл: tests/core_usdt_manual_credit_236.rs
//! Что делает: решение 236, дополнение п. 2 (техлид 09.11) — «Перегнано вручную» спрашивает, сколько USDT зачислено;
//! затраты возврата части = сумма части − зачислено идут в итог сделки, итог дня и итог монеты (как «вернулись»).
//! Ядро на настоящей SQLite во временной папке без площадок в реестре (моков нет). Сделка — как живая
//! 20261104225504576DOGE (bingx → mexc, выручка 83.10 USDT осталась на MEXC: «не вернулись»); зачислено 82.95 — с
//! комиссией вывода USDT BingX 0.15 из справочника (docs/notes/deal-accounting-2026-11-09.md, «BingX 54 × 0.15»).
//! Проверяется: (1) кнопка с вводом и вопрос о зачислении; (2) ответ числом — итог −0.15, день и монета так же,
//! повторное нажатие — без действия; (3) «-» — затраты неизвестны, итог прежний, пометка; (4) не число, меньше нуля,
//! больше суммы части — отказ с причиной, запись остаётся.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::str::FromStr;
use std::time::Duration;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::types::{CommandSource, ManualAction, ManualCommand};
use arb::core::usdt_return_state::UsdtReturn;
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, start, wait_state};

/// Число из строки (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// «Перегнано вручную» из интерфейса с ответом. Возвращает итог ядра.
async fn manual(core: &Core, id: &str, usdt: Option<&str>) -> Result<String, String> {
    core.command(ManualCommand {
        action: ManualAction::UsdtReturnedManually {
            deal_id: id.into(),
            usdt: usdt.map(str::to_string),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

/// Сделка из учёта ядра.
fn deal(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|x| x.id == id).unwrap()
}

/// Закрытая DOGE с «не вернулись» (площадок нет — возврат не состоялся), итог 3.10. Возвращает сумму части.
async fn closed_not_returned(core: &Core, id: &str) -> Decimal {
    let mut x = doge(id);
    arb::core::ledger::finish(core, &mut x, Step::Closed, "продано (выгодно)").await;
    let s = wait_state(core, id, Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    let Some(UsdtReturn::NotReturned { amount, .. }) = s else {
        panic!("ожидалось «не вернулись»: {s:?}");
    };
    assert_eq!(deal(core, id).result_usd, Some(d("3.10")));
    amount
}

#[tokio::test]
async fn credited_answer_goes_into_result_once() {
    let (core, _) = start(&dir("m236c"), None).await;
    let amount = closed_not_returned(&core, "c1DOGE").await;
    assert_eq!(amount, d("83.10"));
    let rec = core
        .manual_records()
        .into_iter()
        .find(|r| r.key == "u.c1DOGE")
        .unwrap();
    let b = rec.button("usdt_manual").unwrap();
    let q = b.input.clone().unwrap();
    assert!(
        q.contains("Сколько USDT зачислено на bingx") && q.contains("«-»"),
        "{q}"
    );
    assert!(
        b.confirm_with(Some("82.95"))
            .contains("Зачислено: 82.95 USDT")
    );
    let t = manual(&core, "c1DOGE", Some("82.95")).await.unwrap();
    assert!(t.contains("зачислено 82.95, затраты возврата 0.15"), "{t}");
    let y = deal(&core, "c1DOGE");
    assert_eq!(y.result_usd, Some(d("2.95")));
    assert_eq!(y.return_cost_usd, Some(d("0.15")));
    assert!(matches!(
        &y.usdt_return,
        Some(UsdtReturn::Manual { credited: Some(c), .. }) if *c == d("82.95")
    ));
    assert_eq!(core.control().day.result_usd, d("2.95"));
    let g = core
        .gas_stats()
        .into_iter()
        .find(|s| s.asset.ticker == "DOGE")
        .unwrap();
    assert_eq!(g.result_usdt, d("2.95"));
    // Повторное нажатие — запись уже решена, итог не меняется.
    let t = manual(&core, "c1DOGE", Some("80")).await.unwrap();
    assert!(t.contains("проигнорирована"), "{t}");
    assert_eq!(deal(&core, "c1DOGE").result_usd, Some(d("2.95")));
    assert_eq!(core.control().day.result_usd, d("2.95"));
}

#[tokio::test]
async fn dash_means_unknown_and_bad_answers_are_refused() {
    let (core, _) = start(&dir("m236u"), None).await;
    closed_not_returned(&core, "c2DOGE").await;
    for bad in ["abc", "-1", "83.11"] {
        let e = manual(&core, "c2DOGE", Some(bad)).await.unwrap_err();
        assert!(
            e.contains("проверьте") || e.contains("не число"),
            "{bad}: {e}"
        );
        assert!(
            matches!(
                deal(&core, "c2DOGE").usdt_return,
                Some(UsdtReturn::NotReturned { .. })
            ),
            "{bad}: запись остаётся"
        );
    }
    manual(&core, "c2DOGE", Some("-")).await.unwrap();
    let y = deal(&core, "c2DOGE");
    assert_eq!(y.result_usd, Some(d("3.10")), "не знает — итог прежний");
    assert!(matches!(
        &y.usdt_return,
        Some(UsdtReturn::Manual { credited: None, .. })
    ));
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("зачисление не указано")),
        "{:?}",
        y.timeline
    );
    assert_eq!(arb::core::deal_return_cost::unknown_parts(&y), 1);
}
