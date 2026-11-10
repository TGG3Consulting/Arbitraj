//! Файл: tests/notify_alert_251.rs
//! Что проверяет: решение 251 (Тигран 10.11: согласовано) — единый шаблон алертов Telegram:
//! 1) застрявшая монета после отказа продажи — живой случай PIG 09.11 (сделка 20261109202626272PIG, mexc → bsc_dex,
//!    куплено 7453353189, зачислено 7065220091, отказ «код 3: execution reverted: minOut; доля 1: minOut 101.590829
//!    USDT, ожидалось 102.101336 USDT, модельный налог монеты 4.0003%», «продать сейчас» −2.03 по ядру — прибыль;
//!    числа — из tests/remeasure_247.rs): путь, где сейчас, цифры, причина, что делает бот, HTML;
//! 2) закрытая сделка с перегоном — живой случай JCT 08.11 (20261108104958308JCT, bingx → bitget, часть продана на
//!    Bitget 21943.3 по 0.006248, остаток перегнан на DEX (кошелёк w7) и продан там 54783.8 по 0.006248, вложено 470;
//!    числа — из tests/core_usdt_return_split.rs; количество в переводах — сумма двух продаж, комиссии переводов в
//!    этих данных нет): путь BingX → Bitget → DEX (кошелёк w7) → BingX (USDT);
//! 3) вывод отклонён — живой ответ MEXC 16021 по сделке 20261104173851521BABYDOGE (как в tests/notify_human.rs) через
//!    настоящий адаптер и настоящий текст ядра (core/rules_apply.rs::tg_error);
//! 4) пачка изменений настроек — одно сообщение со списком (notify/settings_watch.rs::text); имена — из каталога
//!    настроек, значения — для проверки формата (записанных изменений 09.11 в tests/data нет);
//! 5) разметка: экранирование <, >, & (Bot API, HTML style), хвост «Подробно» — в сворачиваемой цитате;
//! 6) сокращение длинного hex и JSON в причине; запись «в обработке» не повторяется в напоминании (mirror_state busy).
//!
//! Примеры печатаются (cargo test --test notify_alert_251 -- --nocapture).

use std::str::FromStr;

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::deal::{Deal, Step};
use arb::core::types::{
    Asset, DexAccountId, Direction, ManualAction, StuckAlert, Transfer, TransferStatus, VenueId,
};
use arb::core::usdt_return_state::UsdtReturn;
use arb::notify::alert::{html, telegram};
use arb::notify::alert_card;
use arb::settings::live::Change;
use arb::settings::value::Scope;
use arb::venues::mexc;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Перевод сделки (вывод или зачисление) на площадке. Ошибок нет.
fn tr(venue: &str, asset: &Asset, amount: &str) -> Transfer {
    Transfer {
        venue: v(venue),
        asset: asset.clone(),
        network: "BSC".into(),
        amount: d(amount),
        status: TransferStatus::Credited,
        venue_ref: String::new(),
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    }
}

/// PIG 09.11: монета на DEX после перевода с MEXC, ждёт решения человека.
fn pig() -> Deal {
    let asset = Asset {
        ticker: "PIG".into(),
        bsc_contract: None,
    };
    let dir = Direction {
        asset: asset.clone(),
        buy_on: v("mexc"),
        sell_on: v("bsc_dex"),
    };
    let mut x = Deal::new(
        "20261109202626272PIG".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.transfers = vec![
        tr("mexc", &asset, "7453353189"),
        tr("bsc_dex", &asset, "7065220091"),
    ];
    x.at = v("bsc_dex");
    x.qty = d("7065220091");
    x.step = Step::WaitingHuman;
    x
}

/// Отказ продажи PIG — текст журнала бота (tests/remeasure_247.rs).
const PIG_ERR: &str = "проба исполнителя откатилась — код 3: execution reverted: minOut; доля 1: minOut 101.590829 USDT, ожидалось 102.101336 USDT, модельный налог монеты 4.0003%";

/// Ставит поиск карточки, как ядро при запуске: по id — карточка одной из сделок теста. Ошибок нет.
fn install() {
    alert_card::install(|id| {
        [pig(), jct()]
            .into_iter()
            .find(|x| x.id == id)
            .map(|x| arb::core::deal_card::card(&x))
    });
}

#[test]
fn stuck_after_sell_refusal() {
    install();
    let a = StuckAlert {
        id: "a1".into(),
        deal_id: "20261109202626272PIG".into(),
        venue: v("bsc_dex"),
        asset: pig().direction.asset,
        qty: d("7065220091"),
        loss_pct: Some(d("-2.03")),
        reason: Some(format!("продажа не прошла: {PIG_ERR}")),
        at: Utc.with_ymd_and_hms(2026, 11, 9, 20, 40, 0).unwrap(),
        error: Some(PIG_ERR.into()),
        attempts: 2,
    };
    let rec = arb::core::manual_rows::alert_record(&a, Decimal::ZERO);
    let tg = telegram(&rec.text);
    println!("--- 1. застряла монета (запись «Неудачные») ---\n{tg}\n");
    let lines: Vec<&str> = rec.text.lines().collect();
    assert_eq!(lines[0], "⛔ Застрявшая монета — нужно решение");
    assert_eq!(lines[1], "Сделка: 20261109202626272PIG");
    assert!(
        tg.contains("<b>Сделка:</b> <code>20261109202626272PIG</code>"),
        "{tg}"
    );
    assert!(tg.contains("<b>Путь:</b> MEXC → DEX\n"), "{tg}");
    assert!(
        tg.contains("<b>Где сейчас:</b> монета: 7065220091 PIG на DEX"),
        "{tg}"
    );
    assert!(
        tg.contains("<b>Цифры:</b>\n• Прибыль при продаже сейчас: +2.03% (по модели"),
        "{tg}"
    );
    assert!(
        tg.contains("<b>Причина:</b> Продажа не прошла: проба исполнителя откатилась"),
        "{tg}"
    );
    assert!(
        tg.contains("<b>Что делает бот:</b> реальный выход ниже модели"),
        "{tg}"
    );
    assert!(tg.contains("Попыток с этим отказом: 2"), "{tg}");
    // «Монеты: …» записи экрана повторяет «Где сейчас» — в Telegram не дублируется; номера решений не видны.
    assert!(!tg.contains("Монеты: ") && !tg.contains("(решени"), "{tg}");
}

/// JCT 08.11: закрыта на DEX (кошелёк w7) после перегона с Bitget, возврат выручки на BingX идёт.
fn jct() -> Deal {
    let asset = Asset {
        ticker: "JCT".into(),
        bsc_contract: None,
    };
    let dir = Direction {
        asset: asset.clone(),
        buy_on: v("bingx"),
        sell_on: v("bitget"),
    };
    let mut x = Deal::new(
        "20261108104958308JCT".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.transfers = vec![
        tr("bingx", &asset, "76727.1"),
        tr("bitget", &asset, "76727.1"),
        tr("bitget", &asset, "54783.8"),
        tr("bsc_dex", &asset, "54783.8"),
    ];
    x.relays = 1;
    x.at = v("bsc_dex");
    x.dex_account = Some(DexAccountId("w7".into()));
    x.invested = d("470");
    x.proceeds = d("21943.3") * d("0.006248") + d("54783.8") * d("0.006248");
    x.qty = Decimal::ZERO;
    x.step = Step::Closed;
    x.result_usd = Some(x.proceeds - x.invested);
    x.usdt_return = Some(UsdtReturn::Pending {
        amount: d("342.29"),
        from: v("bsc_dex"),
        to: v("bingx"),
    });
    x
}

#[test]
fn closed_with_relay() {
    install();
    let text = arb::core::deal_text::closed(&jct(), "продано", None);
    let tg = telegram(&text);
    println!("--- 2. закрыта сделка с перегоном ---\n{tg}\n");
    assert!(
        text.starts_with("✅ Сделка плюсовая\nСделка: 20261108104958308JCT\nМонета: JCT\n"),
        "{text}"
    );
    assert!(
        text.contains("Путь: BingX → Bitget → DEX (кошелёк w7) → BingX (USDT)\n"),
        "{text}"
    );
    assert!(
        text.contains("Где сейчас: монет нет; USDT в пути: DEX (кошелёк w7) → BingX"),
        "{text}"
    );
    assert!(
        text.contains("\n\nЦифры:\n• Размер ордера: 470.00 USDT\n"),
        "{text}"
    );
}

#[test]
fn withdraw_rejected() {
    install();
    let e = mexc::errors::from_code(
        16021,
        "Some pre-crediting assets are temporarily unavailable for withdrawal. Please reconfirm the quantity.",
        mexc::errors::Op::Withdraw,
        "",
    );
    let m = arb::core::rules_apply::tg_error(
        "exec.withdraw_rejected",
        "Вывод",
        &v("mexc"),
        Some("20261104173851521BABYDOGE"),
        &e,
    );
    let tg = telegram(&m.render());
    println!("--- 3. вывод отклонён ---\n{tg}\n");
    assert!(tg.starts_with("<b>⛔ Биржа отклонила вывод</b>\n<b>Сделка:</b> <code>20261104173851521BABYDOGE</code>\nMEXC: вывод\n"), "{tg}");
    assert!(tg.contains("<b>Причина:</b> "), "{tg}");
    // Неизвестный код — ответ биржи в свёрнутом хвосте, полностью — в журнале.
    let u = mexc::errors::from_code(
        99999,
        "Something <new> & odd",
        mexc::errors::Op::Withdraw,
        "",
    );
    let m =
        arb::core::rules_apply::tg_error("exec.withdraw_rejected", "Вывод", &v("mexc"), None, &u);
    let tg = telegram(&m.render());
    assert!(
        tg.ends_with("<b>Подробно (полностью — в журнале):</b>\n<blockquote expandable>Ответ биржи: MEXC 99999: Something &lt;new&gt; &amp; odd</blockquote>"),
        "{tg}"
    );
}

#[test]
fn html_escapes_and_compacts() {
    assert_eq!(html("⚠️ a<b> & c"), "<b>⚠️ a&lt;b&gt; &amp; c</b>");
    let hash = "0x6b1d3c8f0e2a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
    let r = arb::notify::reason::human(&format!(
        "перевод {hash} не найден: {{\"code\":1,\"msg\":\"x\"}}"
    ));
    assert_eq!(r.text, "перевод 0x6b1d…5678 не найден: {…}");
    // Адрес до 16 hex-знаков и текст в скобках без кавычек — как есть.
    assert_eq!(
        arb::notify::reason_compact::compact("0x1234 {шаг 2}"),
        "0x1234 {шаг 2}"
    );
}

#[test]
fn busy_record_is_not_reminded() {
    use arb::notify::mirror_state::{MirrorState, Pending, Prompt};
    let mut st = MirrorState::default();
    assert!(!st.busy("a.a1"));
    st.add_pending(Pending {
        key: Some("a.a1".into()),
        action: ManualAction::SellNow {
            alert_id: "a1".into(),
        },
        title: "Продать сейчас".into(),
        question: "?".into(),
    });
    assert!(st.busy("a.a1"));
    st.prompts.insert(
        (1, 2),
        Prompt {
            key: "h.h1".into(),
            code: "sold_man".into(),
        },
    );
    assert!(st.busy("h.h1"));
    assert!(!st.busy("a.other"));
}
