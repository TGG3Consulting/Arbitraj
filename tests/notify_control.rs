//! Файл: tests/notify_control.rs
//! Что делает: проверяет чат «Управление» (решение 176) без обращения к Telegram: экраны (и «📊 Отчёты», решение 215) и callback_data (≤ 64 байт),
//! диалог перевода USDT от «Откуда» до сводки (списки, проверка суммы: больше нуля, ≤ можно вывести, ≥ минимума вывода,
//! дойдёт ≥ минимума депозита), «Отправить» — команда ядра TransferUsdt, повторное нажатие по устаревшей отрисовке —
//! без действия, номера после перезапуска не совпадают со старыми, лайв-обновление не чаще настройки и заморозка.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::notify. Моков нет.

use std::time::{Duration, Instant};

use arb::core::types::{DexAccountId, ManualAction, VenueId};
use arb::core::usdt_sources::{Source, from_list, to_list};
use arb::core::usdt_transfer_check::{End, Limits, check_amount};
use arb::core::usdt_transfer_plan::account;
use arb::notify::callback::CALLBACK_DATA_MAX_BYTES;
use arb::notify::ctl::live::{Act, plan};
use arb::notify::ctl::sessions::{CtlState, Ref, Sess, View};
use arb::notify::ctl::{transfer, view};
use arb::notify::mirror_state::{MirrorState, Pending, start_seed};
use rust_decimal::Decimal;
use serde_json::Value;

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

fn cex(v: &str) -> End {
    End {
        venue: VenueId(v.into()),
        wallet: None,
    }
}

fn wallet(w: &str) -> End {
    End {
        venue: VenueId("bsc_dex".into()),
        wallet: Some(DexAccountId(w.into())),
    }
}

fn src(end: End, usdt: Option<&str>) -> Source {
    Source {
        label: end.key(),
        end,
        usdt: usdt.map(d),
        error: None,
    }
}

/// Все callback_data клавиатуры.
fn datas(rows: &[Vec<Value>]) -> Vec<String> {
    rows.iter()
        .flatten()
        .filter_map(|b| b["callback_data"].as_str().map(String::from))
        .collect()
}

#[test]
fn transfer_lists_from_and_to() {
    let all = vec![
        src(cex("bingx"), Some("120.5")),
        src(cex("mexc"), Some("0")),
        src(wallet("w1"), Some("30")),
        src(wallet("w2"), None),
    ];
    let from: Vec<String> = from_list(&all).iter().map(|s| s.end.key()).collect();
    assert_eq!(from, ["bingx", "bsc_dex/w1"]);
    let to: Vec<String> = to_list(&all, &cex("bingx"))
        .iter()
        .map(|s| s.end.key())
        .collect();
    assert_eq!(to, ["mexc", "bsc_dex/w1", "bsc_dex/w2"]);
    // От кошелька — только биржи: кошелёк → кошелёк ядро не переводит.
    let to: Vec<String> = to_list(&all, &wallet("w1"))
        .iter()
        .map(|s| s.end.key())
        .collect();
    assert_eq!(to, ["bingx", "mexc"]);
    assert!(account(&wallet("w1"), &wallet("w2")).is_err());
    assert!(account(&cex("bingx"), &cex("bingx")).is_err());
    assert_eq!(
        account(&cex("bingx"), &wallet("w1")).unwrap(),
        Some(DexAccountId("w1".into()))
    );
    assert_eq!(account(&cex("bingx"), &cex("mexc")).unwrap(), None);
    // Экран шага 1: кнопки «t:<номер>.f<i>» в пределе протокола.
    let (s, refs) = transfer::from(&all, &from_list(&all), u64::MAX, None);
    let data = datas(&s.rows);
    assert!(data.contains(&format!("t:{}.f0", u64::MAX)));
    assert!(data.iter().all(|x| x.len() <= CALLBACK_DATA_MAX_BYTES));
    assert_eq!(refs, vec![Ref::End(cex("bingx")), Ref::End(wallet("w1"))]);
}

#[test]
fn amount_checks() {
    let l = Limits {
        withdrawable: d("100"),
        min: d("10"),
        fee: d("1"),
        deposit_min: d("5"),
    };
    let r = ("bingx", "mexc");
    assert_eq!(check_amount(d("50"), &l, "USDT", r), Ok(d("49")));
    assert_eq!(check_amount(d("100"), &l, "USDT", r), Ok(d("99")));
    assert!(
        check_amount(d("100.01"), &l, "USDT", r)
            .unwrap_err()
            .contains("больше, чем можно вывести")
    );
    assert!(
        check_amount(d("9.99"), &l, "USDT", r)
            .unwrap_err()
            .contains("меньше минимума вывода bingx")
    );
    assert!(
        check_amount(d("0"), &l, "USDT", r)
            .unwrap_err()
            .contains("ноль")
    );
    let tight = Limits {
        min: d("1"),
        fee: d("1"),
        deposit_min: d("5"),
        ..l
    };
    assert!(
        check_amount(d("5.5"), &tight, "USDT", r)
            .unwrap_err()
            .contains("минимума депозита mexc")
    );
}

/// Экран диалога в номере.
fn sess(view: View) -> Sess {
    Sess {
        chat: -1002,
        msg: 7,
        view,
        refs: vec![],
        dialog: None,
        touched: Instant::now(),
        shown: String::new(),
        frozen: false,
        edited: None,
    }
}

#[test]
fn stale_render_never_moves_money() {
    let ttl = Duration::from_secs(600);
    let mut st = CtlState::default();
    let a = st.reserve();
    st.put(a, sess(View::Transfer));
    assert!(st.get(a, ttl).is_some());
    // Тот же экран перерисован — прежние кнопки устарели.
    let b = st.reserve();
    st.put(b, sess(View::Transfer));
    assert!(b > a);
    assert!(st.get(a, ttl).is_none());
    // Диалог закрыт «Отправить» — второе нажатие не находит отрисовку.
    st.sess.remove(&b);
    assert!(st.get(b, ttl).is_none());
    // Номера — с времени запуска: старые (до перезапуска) меньше любого нового.
    assert!(a > start_seed());
    // Подтверждение «Да»: второй раз не находится (уже выполнено), номер — тоже с времени запуска.
    let mut m = MirrorState::default();
    let p = Pending {
        key: None,
        title: "Перевести USDT".into(),
        question: "Перевести 10 USDT с bingx на mexc?".into(),
        action: ManualAction::TransferUsdt {
            from: "bingx".into(),
            from_wallet: None,
            to: "mexc".into(),
            to_wallet: None,
            amount: "10".into(),
        },
    };
    let n = m.add_pending(p);
    assert!(n > start_seed());
    assert!(m.pending.remove(&n).is_some());
    assert!(m.pending.remove(&n).is_none());
}

#[test]
fn live_not_faster_than_setting() {
    let every = Duration::from_secs(5);
    let idle = Duration::from_secs(120);
    let fresh = sess(View::State);
    let just = Sess {
        edited: Some(Instant::now()),
        ..sess(View::Venue("bingx".into()))
    };
    let old = Instant::now()
        .checked_sub(Duration::from_secs(300))
        .unwrap();
    let idle_s = Sess {
        touched: old,
        msg: 8,
        ..sess(View::State)
    };
    let frozen = Sess {
        frozen: true,
        msg: 9,
        ..sess(View::State)
    };
    let dialog = Sess {
        msg: 10,
        ..sess(View::Transfer)
    };
    let list = vec![(1, fresh), (2, just), (3, idle_s), (4, frozen), (5, dialog)];
    assert_eq!(
        plan(&list, every, idle),
        vec![(1, Act::Refresh), (3, Act::Freeze)]
    );
}

#[test]
fn screens_and_codes() {
    assert_eq!(view::parse("bv.bingx"), Some(View::Venue("bingx".into())));
    assert_eq!(
        view::parse("bw.0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90"),
        Some(View::Wallet("0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90".into()))
    );
    assert_eq!(view::parse("tr"), Some(View::Transfer));
    assert_eq!(view::parse("zz"), None);
    assert_eq!(view::parse("bv."), None);
    let r = arb::core::risk::Risk::default();
    let root = arb::notify::ctl::screens::root(&r);
    assert_eq!(
        datas(&root.rows),
        ["k:st", "k:bal", "k:tr", "k:man", "r:m", "k:root"]
    );
    // Решение 215: «📊 Отчёты» → отчёт → период; callback_data в пределе 64 байт.
    let rep = arb::notify::ctl::report::choose();
    assert_eq!(
        datas(&rep.rows),
        ["r:res", "r:mis", "r:sig", "r:cap", "k:root"]
    );
    // Решение 255: «Сигналы и сделки» — свои периоды и «Свой интервал».
    let sig = arb::notify::ctl::report::periods("sig").expect("экран периода");
    assert_eq!(
        datas(&sig.rows),
        [
            "r:sig.a", "r:sig.t", "r:sig.3", "r:sig.5", "r:sig.7", "r:sig.c", "r:m", "k:root"
        ]
    );
    // Решение 256: «Сделки, лимиты и капитал» — свои периоды и свой интервал.
    let cap = arb::notify::ctl::report::periods("cap").expect("экран периода");
    assert_eq!(
        datas(&cap.rows),
        [
            "r:cap.a", "r:cap.t", "r:cap.3", "r:cap.5", "r:cap.7", "r:cap.c", "r:m", "k:root"
        ]
    );
    let per = arb::notify::ctl::report::periods("mis").expect("экран периода");
    assert_eq!(
        datas(&per.rows),
        ["r:mis.d", "r:mis.w", "r:mis.m", "r:mis.a", "r:m", "k:root"]
    );
    assert!(arb::notify::ctl::report::periods("zz").is_none());
    let st = arb::notify::ctl::screens::state(&r, 0);
    let data = datas(&st.rows);
    // После запуска — «Стоп»: доступен «Старт»; сброс лимита дня; «Обновить», «Назад», «Главное меню».
    for x in ["c:mode.Running", "c:dr.x", "k:st", "k:root"] {
        assert!(data.contains(&x.to_string()), "{x}: {data:?}");
    }
    assert_eq!(
        arb::notify::ctl::screens::message_link(-1001234567890, 55).as_deref(),
        Some("https://t.me/c/1234567890/55")
    );
    assert_eq!(arb::notify::ctl::screens::message_link(-4567, 55), None);
}
