//! Файл: tests/core_deal_accounting_236.rs
//! Что делает: решение 236 на настоящей SQLite во временной папке (ядро без площадок в реестре, как tests/core_usdt_return_*;
//! моков нет). Данные — живые сделки бота (GET /api/deals, 08.11.2026):
//! (1) 20261108142947740PIT (mexc → DEX): бот продал на DEX на 44.8595 USDT, остаток 729.325787234 PIT человек закрыл
//!     «Продано вручную», введя 44.88 (вся сделка) — ядро прибавило ввод к выручке бота, итог +44.78 при плане +0.01.
//!     Проверяется: вопрос и подтверждение «Продано вручную» называют выручку бота и просят только свою; честный ввод
//!     (0 за 729 PIT) даёт итог = выручка бота − вложено − затраты; хронология показывает обе части.
//! (2) Затраты возврата USDT в итоге сделки: 20261108205335162HELLO (mexc → DEX: запрошено 27.655454087068799999999999694,
//!     зачислено 27.59; точность вывода USDT у MEXC — 0.1, ушло 27.6, комиссия 0.01) — затраты = ушло − зачислено
//!     (0.01), остаток округления 0.0554… не затрата; запись до решения 236 (ушло неизвестно) — amount − credited.
//!     Часть «вернулись» уменьшает итог сделки, итог дня и итог монеты ровно на затраты, повтор не вычитает дважды;
//!     старая запись исправляется при загрузке в памяти, база не меняется.
//! (3) Газ возврата не в USDT: затрата в USDT — уже в разнице; газ BNB без живой цены — ошибка (в итог не входит).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets.

mod usdt_return_common;

use std::str::FromStr;
use std::time::Duration;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deal_return_cost::{self as rc, Relayed};
use arb::core::manual_feed::halted_record;
use arb::core::types::{
    CommandSource, Cost, CostKind, DexAccountId, Fee, ManualAction, ManualCommand, Side,
    TradeRequest, TradeResult, TradeStatus, VenueId,
};
use arb::core::usdt_return_state::{self as st, UsdtReturn};
use rust_decimal::Decimal;
use usdt_return_common::{dir, doge, start, wait_state};

/// Число из строки. Ошибок нет (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Площадка по id.
fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Продажа бота на DEX (данные PIT из /api/deals): qty по avg, комиссия — газ BNB. Ошибок нет.
fn dex_sell(qty: &str, avg: &str, gas_bnb: &str) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: v("bsc_dex"),
            asset: doge("x").direction.asset,
            side: Side::Sell,
            qty: d(qty),
            worst_price: d(avg),
            deal_id: "x".into(),
            route: None,
            exit_position: true,
            remeasure: None,
        },
        status: TradeStatus::PartiallyFilled,
        filled_qty: d(qty),
        avg_price: d(avg),
        fees: vec![Fee {
            asset: "BNB".into(),
            amount: d(gas_bnb),
            usd: None,
        }],
        venue_ref: "0x".into(),
        chain: None,
        finished_at: chrono::Utc::now(),
        executor: None,
    }
}

/// Сделка как живая 20261108142947740PIT перед «Продано вручную»: остановлена, в сделке остаток 729.325787234,
/// вложено, затраты и две продажи бота на DEX — числа из /api/deals. Ошибок нет.
fn pit(id: &str) -> Deal {
    let mut x = doge(id);
    x.direction.asset.ticker = "PIT".into();
    x.direction.buy_on = v("mexc");
    x.direction.sell_on = v("bsc_dex");
    x.at = v("bsc_dex");
    x.dex_account = Some(DexAccountId("w2".into()));
    x.invested = d("44.883118379626699990152816303");
    x.spent = d("0.07218496876970136000");
    x.sells = vec![
        dex_sell(
            "290039572024.934400000",
            "0.000000000154666925577898067",
            "0.0000265327",
        ),
        dex_sell(
            "17503.818893630",
            "0.0000000001546913427813392113",
            "0.0000254848",
        ),
    ];
    x.proceeds = x.sells.iter().map(|r| r.filled_qty * r.avg_price).sum();
    x.qty = d("729.325787234");
    x.step = Step::Halted;
    x
}

/// «Продано вручную» из интерфейса. Возвращает итог ядра.
async fn sold_manually(core: &Core, id: &str, usdt: &str) -> Result<String, String> {
    core.command(ManualCommand {
        action: ManualAction::SoldManually {
            deal_id: id.into(),
            usdt: usdt.into(),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

/// Сделка из учёта ядра. Ошибок нет (тест).
fn deal(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|x| x.id == id).unwrap()
}

#[tokio::test]
async fn pit_sold_manually_asks_only_human_part_and_does_not_double() {
    let (core, _) = start(&dir("a236pit"), None).await;
    let x = pit("a1PIT");
    let bot = x.proceeds;
    assert_eq!(
        bot.round_dp(4),
        d("44.8595"),
        "выручка бота — как в живой сделке"
    );
    arb::core::ledger::save(&core, &x).await;
    // Вопрос и подтверждение называют выручку бота (44.86 USDT) и просят только выручку человека.
    let rec = halted_record(&x);
    let b = rec.button("sold_man").unwrap();
    let input = b.input.clone().unwrap();
    assert!(
        input.contains("44.86 USDT") && input.contains("их не вводите"),
        "{input}"
    );
    assert!(input.contains("729.325787234 PIT"), "{input}");
    assert!(
        b.confirm.contains("плюс выручка бота 44.86 USDT"),
        "{}",
        b.confirm
    );
    assert!(rec.text.contains("Выручка бота"), "{}", rec.text);
    // Честный ввод: за 729 PIT человек получил 0 — итог = выручка бота − вложено − затраты (≈ −0.0958), не +44.78.
    let text = sold_manually(&core, "a1PIT", "0").await.unwrap();
    let y = deal(&core, "a1PIT");
    let want = bot - x.invested - x.spent;
    assert_eq!(y.result_usd, Some(want));
    assert_eq!(want.round_dp(4), d("-0.0958"));
    assert!(
        text.contains("выручка бота") && text.contains("итог -0.0958"),
        "{text}"
    );
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("выручка человека 0 USDT (ввод) + выручка продаж бота")),
        "{:?}",
        y.timeline
    );
}

#[test]
fn hello_cost_is_fee_not_rounding_rest() {
    // Новая запись: ушло 27.6 (точность вывода 0.1), зачислено 27.59 — затраты 0.01; остаток 0.0554… на MEXC.
    let r = Relayed {
        sent: d("27.6"),
        credited: d("27.59"),
        fee_usd: None,
        fee_note: None,
    };
    let amount = d("27.655454087068799999999999694");
    let part = r.done(amount, v("mexc"), v("bsc_dex"));
    assert_eq!(rc::part_cost(&part), Some(d("0.01")));
    assert!(
        r.tail().contains("затраты возврата 0.01 USDT"),
        "{}",
        r.tail()
    );
    // Запись до решения 236 (ушло неизвестно): затраты — amount − credited, другого факта в записи нет.
    let old = UsdtReturn::Done {
        amount,
        credited: d("27.59"),
        from: v("mexc"),
        to: v("bsc_dex"),
        sent: None,
        fee_usd: None,
    };
    assert_eq!(rc::part_cost(&old), Some(amount - d("27.59")));
    // Газ кошелька DEX (в USDT) — добавляется.
    let gas = Relayed {
        sent: d("44.93"),
        credited: d("44.93"),
        fee_usd: Some(d("0.0021")),
        fee_note: None,
    };
    assert_eq!(
        rc::part_cost(&gas.done(d("44.93"), v("bsc_dex"), v("bitget"))),
        Some(d("0.0021"))
    );
    // «Не вернулись» / «перегнано вручную» — стоимости нет.
    let nr = UsdtReturn::NotReturned {
        amount,
        at: v("mexc"),
        why: "x".into(),
    };
    assert_eq!(rc::part_cost(&nr), None);
}

#[tokio::test]
async fn done_part_lowers_deal_day_and_coin_result_once() {
    let (core, _) = start(&dir("a236done"), None).await;
    let mut x = doge("a2DOGE");
    // Закрытие: возврат mexc → bingx не состоится (площадок нет) — «не вернулись», итог без затрат возврата.
    arb::core::ledger::finish(&core, &mut x, Step::Closed, "продано (выгодно)").await;
    wait_state(&core, "a2DOGE", Duration::from_secs(5), |s| {
        matches!(s, UsdtReturn::NotReturned { .. })
    })
    .await;
    let y = deal(&core, "a2DOGE");
    assert_eq!(y.result_usd, Some(d("3.10")));
    assert_eq!(
        y.closed_notice, None,
        "возврат закончен — отложенное deal.closed ушло"
    );
    assert_eq!(core.control().day.result_usd, d("3.10"));
    // «Перегнать» прошло: ушло 83.1, зачислено 82.95 (комиссия 0.15) + газ 0.02 USDT — итог 3.10 − 0.17.
    let r = Relayed {
        sent: d("83.1"),
        credited: d("82.95"),
        fee_usd: Some(d("0.02")),
        fee_note: None,
    };
    st::set(
        &core,
        "a2DOGE",
        r.done(d("83.10"), v("mexc"), v("bingx")),
        "вернулись",
    )
    .await;
    let y = deal(&core, "a2DOGE");
    assert_eq!(y.result_usd, Some(d("2.93")));
    assert_eq!(y.return_cost_usd, Some(d("0.17")));
    assert_eq!(core.control().day.result_usd, d("2.93"));
    let g = core
        .gas_stats()
        .into_iter()
        .find(|s| s.asset.ticker == "DOGE")
        .unwrap();
    assert_eq!(g.result_usdt, d("2.93"));
    // Та же часть ещё раз — второй раз не вычитается.
    st::set(
        &core,
        "a2DOGE",
        r.done(d("83.10"), v("mexc"), v("bingx")),
        "",
    )
    .await;
    assert_eq!(deal(&core, "a2DOGE").result_usd, Some(d("2.93")));
    assert_eq!(core.control().day.result_usd, d("2.93"));
}

#[tokio::test]
async fn old_record_fixed_in_memory_on_load() {
    let p = dir("a236old");
    {
        let (core, _) = start(&p, None).await;
        let mut x = doge("a3DOGE");
        x.step = Step::Closed;
        x.result_usd = Some(d("3.10"));
        // Запись до решения 236: возврат «вернулись», итог без затрат возврата, полей sent / return_cost_usd нет.
        x.usdt_return = Some(UsdtReturn::Done {
            amount: d("83.10"),
            credited: d("82.95"),
            from: v("mexc"),
            to: v("bingx"),
            sent: None,
            fee_usd: None,
        });
        arb::core::ledger::save(&core, &x).await;
    }
    for _ in 0..2 {
        let (core, _) = start(&p, None).await;
        let y = deal(&core, "a3DOGE");
        assert_eq!(y.result_usd, Some(d("2.95")), "каждый запуск — тот же итог");
        assert_eq!(y.return_cost_usd, Some(d("0.15")));
        // Итог дня при загрузке не меняется — поправка старой сделки только в её итоге.
        assert_eq!(core.control().day.result_usd, Decimal::ZERO);
    }
}

#[tokio::test]
async fn fee_in_quote_is_inside_difference_bnb_needs_price() {
    let (core, _) = start(&dir("a236fee"), None).await;
    let usdt = Cost::new(CostKind::WithdrawFee, d("0.15"), "USDT", false);
    assert_eq!(
        rc::fee_usd(&core, &v("bingx"), "USDT", Some(&usdt)).await,
        Ok(None)
    );
    assert_eq!(
        rc::fee_usd(&core, &v("bingx"), "USDT", None).await,
        Ok(None)
    );
    // Газ BNB кошелька: живой цены BNB нет (площадок нет) — ошибка, в итог не входит (Relayed.fee_note).
    let gas = Cost::new(CostKind::TransferGas, d("0.000003"), "BNB", false);
    assert!(
        rc::fee_usd(&core, &v("bsc_dex"), "USDT", Some(&gas))
            .await
            .is_err()
    );
}
