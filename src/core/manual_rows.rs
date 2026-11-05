//! Файл: core/manual_rows.rs
//! Что делает: записи «Ручного управления» с кнопками для алертов застрявших монет и монет «Беру на себя»
//! (решение 156; продолжение manual_feed.rs — размер файла). Кнопки, команды и вопросы подтверждения — ровно как
//! на экране (ui/src/pages/Manual.svelte до решения 156), «Продано вручную» — с вводом выручки USDT.
//! Кто вызывает: core/manual_feed.rs.
//! От чего зависит: core/types/manual_feed.rs, core/types/control.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human::{self as hu, Msg};

use crate::core::types::{
    ManualAction as A, ManualButton, ManualHolding, ManualRecord, RecordKind, StuckAlert,
};

/// Время для текста записи.
pub fn when(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// Кнопка без ввода. Принимает: код, надпись, команду, вопрос, недоступность, второстепенная ли.
pub fn btn(
    code: &str,
    title: &str,
    action: A,
    confirm: String,
    disabled: Option<String>,
    quiet: bool,
) -> ManualButton {
    ManualButton {
        code: code.into(),
        title: title.into(),
        action,
        confirm,
        input: None,
        disabled,
        quiet,
    }
}

/// «Продано вручную» — общая для всех видов записей (ввод выручки USDT, как prompt на экране).
pub fn sold(deal_id: &str, ticker: &str, qty: Decimal) -> ManualButton {
    ManualButton {
        code: "sold_man".into(),
        title: "Продано вручную".into(),
        action: A::SoldManually {
            deal_id: deal_id.into(),
            usdt: String::new(),
        },
        confirm: format!("Закрыть сделку {deal_id}: выручка {{usdt}} USDT?"),
        input: Some(format!(
            "Сколько USDT вы получили за {qty} {ticker}? Сделка {deal_id} закроется этой выручкой."
        )),
        disabled: None,
        quiet: true,
    }
}

/// Запись алерта: «Продать сейчас», «Продать и перегнать», «Беру на себя», «Продано вручную».
pub fn alert_record(a: &StuckAlert) -> ManualRecord {
    let (t, v, q) = (&a.asset.ticker, &a.venue.0, a.qty);
    let id = a.id.clone();
    let loss = a.loss_pct.map(hu::pct).unwrap_or_else(|| "—".into());
    let full = a.reason.clone().unwrap_or_default();
    let coins = format!("{} {t} на {}", hu::qty(q), hu::venue(v));
    ManualRecord {
        key: ManualRecord::key_of(RecordKind::Alert, &a.id),
        kind: RecordKind::Alert,
        object_id: a.id.clone(),
        deal_id: a.deal_id.clone(),
        venue: v.clone(),
        text: Msg::new(hu::STOP, "Застрявшая монета — нужно решение")
            .kv("Монеты", &coins)
            .kv("Убыток при продаже сейчас", &loss)
            .reason(&full)
            // Решение 177: тот же отказ по этой записи — счётчик попыток вместо нового алерта (manual_repeat.rs).
            .line(if a.attempts > 1 {
                format!("Попыток с этим отказом: {}", a.attempts)
            } else {
                String::new()
            })
            .kv("Когда", when(a.at))
            .id(&a.deal_id)
            .render(),
        reason: crate::notify::reason::short(&full),
        reason_full: full.clone(),
        buttons: vec![
            btn(
                "sell_now",
                "Продать сейчас",
                A::SellNow {
                    alert_id: id.clone(),
                },
                format!("Продать {q} {t} на {v} по рынку сейчас?"),
                None,
                false,
            ),
            btn(
                "sell_ret",
                "Продать и перегнать",
                A::SellNowReturn {
                    alert_id: id.clone(),
                },
                format!(
                    "Продать {q} {t} на {v} по рынку и перегнать выручку USDT на площадку покупки?"
                ),
                None,
                false,
            ),
            btn(
                "take_over",
                "Беру на себя",
                A::TakeOver { alert_id: id },
                format!("Забрать {q} {t} на себя? Бот спишет их из учёта."),
                None,
                true,
            ),
            sold(&a.deal_id, t, q),
        ],
    }
}

/// Запись «Беру на себя»: «Вернуть боту», «Продано вручную».
pub fn holding_record(h: &ManualHolding) -> ManualRecord {
    let (t, v) = (&h.asset.ticker, &h.venue.0);
    ManualRecord {
        key: ManualRecord::key_of(RecordKind::Holding, &h.id),
        kind: RecordKind::Holding,
        object_id: h.id.clone(),
        deal_id: h.deal_id.clone(),
        venue: v.clone(),
        text: Msg::new(hu::INFO, "Монеты у вас («Беру на себя»)")
            .kv(
                "Монеты",
                format!("{} {t} на {}", hu::qty(h.qty), hu::venue(v)),
            )
            .reason(&h.reason)
            .kv("Взяты", when(h.taken_at))
            .id(&h.deal_id)
            .render(),
        reason: crate::notify::reason::short(&h.reason),
        reason_full: h.reason.clone(),
        buttons: vec![
            btn(
                "return_bot",
                "Вернуть боту",
                A::ReturnToBot {
                    holding_id: h.id.clone(),
                },
                format!("Вернуть {t} боту? Учёт — по реальному балансу {v}."),
                None,
                false,
            ),
            sold(&h.deal_id, t, h.qty),
        ],
    }
}
