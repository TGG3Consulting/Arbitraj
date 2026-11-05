//! Файл: tests/notify_mirror.rs
//! Что делает: проверяет дубль «Ручного управления» в Telegram (решение 156) без обращения к Telegram:
//! кнопки записи в Telegram = кнопки записи экрана (для каждого вида записи: алерт, «Беру на себя», остановленная
//! сделка), недоступные на экране кнопки в Telegram не показываются (причина — в тексте), команда кнопки — та же,
//! что слал экран; напоминание каждые N секунд и прекращение после того, как запись ушла; итог «выполнено»
//! находит сообщения ушедшей записи; второе «Да» по тому же вопросу — «уже выполнено».
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (типы, manual_feed, manual_rows), arb::notify. Моков нет; записанных ответов Telegram
//! в репозитории нет (нужен токен бота) — разбор ответов Telegram здесь не проверяется.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use arb::core::deal::{Deal, Step};
use arb::core::manual_feed::halted_record;
use arb::core::manual_rows::{alert_record, holding_record};
use arb::core::types::{
    Asset, Direction, ManualAction, ManualHolding, ManualRecord, StuckAlert, VenueId,
};
use arb::notify::mirror_state::{MirrorState, Pending};
use arb::notify::mirror_view::{batch_message, record_message};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

/// Направление на тикере из записи BingX (tests/data/bingx/symbols_spot_2026-10-23.json).
fn cake() -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

fn alert() -> StuckAlert {
    StuckAlert {
        id: "0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90".into(),
        deal_id: "20261104120000123CAKE".into(),
        venue: VenueId("bsc_dex".into()),
        asset: cake().asset,
        qty: Decimal::new(1234, 2),
        loss_pct: Some(Decimal::new(-35, 1)),
        reason: Some("убыток 3.5% выше порога 2%".into()),
        at: Utc::now(),
        error: None,
        attempts: 1,
    }
}

fn holding() -> ManualHolding {
    ManualHolding {
        id: "5d1e7a20-8c3b-4b6f-a2d9-0e4f6b8c1d33".into(),
        deal_id: "20261104120000123CAKE".into(),
        venue: VenueId("bsc_dex".into()),
        asset: cake().asset,
        qty: Decimal::new(1234, 2),
        reason: "алерт: убыток".into(),
        taken_at: Utc::now(),
    }
}

/// Остановленная сделка: at — где монеты, qty — сколько.
fn halted(at: &str, qty: Decimal) -> Deal {
    let mut d = Deal::new(
        "20261104120000123CAKE".into(),
        cake(),
        json!(null),
        Decimal::ONE,
    );
    d.step = Step::Halted;
    d.at = VenueId(at.into());
    d.qty = qty;
    d
}

/// (надпись, callback_data) кнопок клавиатуры.
fn kb_buttons(kb: &Option<Value>) -> Vec<(String, String)> {
    let Some(kb) = kb else { return vec![] };
    kb["inline_keyboard"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r.as_array().unwrap().clone())
        .map(|b| {
            (
                b["text"].as_str().unwrap().to_string(),
                b["callback_data"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Кнопки Telegram = доступные кнопки записи экрана (надписи и коды), недоступные — в тексте с причиной.
fn same_as_screen(r: &ManualRecord) {
    let (text, kb) = record_message(r);
    let tg: BTreeSet<(String, String)> = kb_buttons(&kb).into_iter().collect();
    let screen: BTreeSet<(String, String)> = r
        .buttons
        .iter()
        .filter(|b| b.disabled.is_none())
        .map(|b| (b.title.clone(), format!("{}:{}", b.code, r.key)))
        .collect();
    assert_eq!(tg, screen, "{}", r.key);
    for b in r.buttons.iter().filter(|b| b.disabled.is_some()) {
        assert!(text.contains(b.disabled.as_ref().unwrap()), "{text}");
    }
    for (_, data) in &tg {
        assert!(data.len() <= 64, "{data}");
    }
}

#[test]
fn alert_buttons_match_screen() {
    let r = alert_record(&alert());
    let titles: Vec<&str> = r.buttons.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Продать сейчас",
            "Продать и перегнать",
            "Беру на себя",
            "Продано вручную"
        ]
    );
    let id = alert().id;
    assert_eq!(
        r.button("sell_now").unwrap().action,
        ManualAction::SellNow {
            alert_id: id.clone()
        }
    );
    assert_eq!(
        r.button("sell_ret").unwrap().action,
        ManualAction::SellNowReturn {
            alert_id: id.clone()
        }
    );
    assert_eq!(
        r.button("take_over").unwrap().action,
        ManualAction::TakeOver { alert_id: id }
    );
    let sm = r.button("sold_man").unwrap();
    assert!(sm.input.is_some());
    assert_eq!(
        sm.action_with(Some(" 24.44 ")),
        ManualAction::SoldManually {
            deal_id: alert().deal_id,
            usdt: "24.44".into()
        }
    );
    assert_eq!(
        sm.confirm_with(Some("24.44")),
        "Закрыть сделку 20261104120000123CAKE: выручка 24.44 USDT?"
    );
    same_as_screen(&r);
}

#[test]
fn holding_buttons_match_screen() {
    let r = holding_record(&holding());
    let titles: Vec<&str> = r.buttons.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["Вернуть боту", "Продано вручную"]);
    assert_eq!(
        r.button("return_bot").unwrap().action,
        ManualAction::ReturnToBot {
            holding_id: holding().id
        }
    );
    same_as_screen(&r);
}

#[test]
fn halted_buttons_match_screen_and_disabled() {
    let r = halted_record(&halted("bsc_dex", Decimal::TEN));
    let titles: Vec<&str> = r.buttons.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Продать сейчас", "Продать и перегнать", "Продано вручную"]
    );
    assert!(r.buttons.iter().all(|b| b.disabled.is_none()));
    same_as_screen(&r);
    // Монеты уже на площадке покупки — «Продать и перегнать» недоступна (как на экране).
    let r = halted_record(&halted("bingx", Decimal::TEN));
    assert!(r.button("sell_ret").unwrap().disabled.is_some());
    assert!(r.button("sell_now").unwrap().disabled.is_none());
    same_as_screen(&r);
    // Монет нет — обе продажи недоступны, остаётся «Продано вручную».
    let r = halted_record(&halted("bsc_dex", Decimal::ZERO));
    let (_, kb) = record_message(&r);
    assert_eq!(kb_buttons(&kb).len(), 1);
    same_as_screen(&r);
}

#[test]
fn batch_has_buttons_of_every_record() {
    let recs = [alert_record(&alert()), holding_record(&holding())];
    let refs: Vec<&ManualRecord> = recs.iter().collect();
    let (text, kb) = batch_message(&refs, "Напоминание");
    let b = kb_buttons(&kb);
    assert_eq!(b.len(), 6);
    assert!(b[0].0.starts_with("1 · ") && b[5].0.starts_with("2 · "));
    assert!(text.contains("1. ") && text.contains("2. "));
}

#[test]
fn repeat_every_n_and_stop_after_action() {
    let every = Some(Duration::from_secs(10));
    let recs = vec![
        alert_record(&alert()),
        halted_record(&halted("bsc_dex", Decimal::TEN)),
    ];
    let mut st = MirrorState::default();
    assert!(st.apply(&recs).gone.is_empty());
    assert_eq!(st.unsent(&recs).len(), 2);
    let t0 = Instant::now();
    st.mark_sent(&recs[0].key, "a".into(), 101, t0);
    st.mark_sent(&recs[1].key, "d".into(), 102, t0);
    assert!(st.unsent(&recs).is_empty());
    assert!(!st.reminder_due(t0 + Duration::from_millis(9_999), every));
    assert!(st.reminder_due(t0 + Duration::from_secs(10), every));
    // Не задано notify.manual_repeat_s — без повторов.
    assert!(!st.reminder_due(t0 + Duration::from_secs(3600), None));
    st.last_reminder = Some(t0 + Duration::from_secs(10));
    assert_eq!(st.next_reminder(every), Some(t0 + Duration::from_secs(20)));
    // Пауза Telegram (retry_after) — не напоминаем до её конца.
    st.pause_until = Some(t0 + Duration::from_secs(25));
    assert!(!st.reminder_due(t0 + Duration::from_secs(21), every));
    // Алерт ушёл с экрана — его сообщение к правке, остался один ждущий.
    let left = vec![recs[1].clone()];
    let diff = st.apply(&left);
    assert_eq!(diff.gone.len(), 1);
    assert_eq!(diff.gone[0].1.msgs, vec![101]);
    assert_eq!(st.waiting(&left).len(), 1);
    // Итог команды находит сообщения ушедшей записи — один раз.
    let a = ManualAction::SellNow {
        alert_id: alert().id,
    };
    assert_eq!(st.take_closed(&a).len(), 1);
    assert!(st.take_closed(&a).is_empty());
    // Всё разобрано — напоминаний больше нет.
    st.apply(&[]);
    assert_eq!(st.next_reminder(every), None);
}

#[test]
fn second_confirm_does_not_repeat() {
    let recs = vec![alert_record(&alert())];
    let mut st = MirrorState::default();
    st.apply(&recs);
    let b = recs[0].button("sell_now").unwrap();
    let p = Pending {
        key: Some(recs[0].key.clone()),
        action: b.action.clone(),
        title: b.title.clone(),
        question: b.confirm.clone(),
    };
    let n = st.add_pending(p.clone());
    let m = st.add_pending(p);
    // Первое «Да»: ожидание забрано, запись отмечена выполненной (press_confirm.rs) — напоминаний по ней нет.
    assert!(st.pending.remove(&n).is_some());
    st.done.insert(recs[0].key.clone());
    assert!(st.waiting(&recs).is_empty());
    // Повтор того же «Да» — ожидания нет («Уже выполнено или отменено»); второй вопрос — запись уже выполнена.
    assert!(st.pending.remove(&n).is_none());
    assert!(st.done.contains(st.pending[&m].key.as_ref().unwrap()));
    // Запись ушла и вернулась с тем же ключом (продажа не прошла) — снова доступна.
    st.apply(&[]);
    st.apply(&recs);
    assert_eq!(st.waiting(&recs).len(), 1);
}
