//! Файл: notify/ctl/transfer.rs
//! Что делает: экраны диалога «Перевести USDT» чата «Управление» (решение 176) — только рендер: шаг 1 «Откуда»
//! (площадки с USDT: биржи и кошельки DEX по маппингу, решение 132), шаг 2 «Куда» (кроме «откуда»; от кошелька —
//! только биржи), шаг 3 «Сколько» (можно вывести, минимум, комиссия; «Всё», доли notify.transfer_pct, «Ввести сумму»),
//! шаг 4 — сводка «С X на Y: N USDT, комиссия вывода F, дойдёт N − F, сеть …» с «Отправить» / «Отмена».
//! callback_data: «t:<номер отрисовки>.<действие>»: f<i> — откуда, d<i> — куда, a — всё, p<i> — доля, e — ввести,
//! go — отправить, x — отмена, b — назад.
//! Кто вызывает: notify/ctl/transfer_press.rs.
//! От чего зависит: core/usdt_sources.rs, core/usdt_transfer_plan.rs (Quote), ctl/kb.rs, ctl/sessions.rs, notify/human.rs.
//! Настройки: нет (доли — notify.transfer_pct, читает вызывающий).

use rust_decimal::Decimal;

use crate::core::usdt_sources::Source;
use crate::core::usdt_transfer_check::Quote;

use super::kb::{Screen, btn};
use super::sessions::Ref;

/// Число USDT как есть (без округления до 2 знаков: это сумма перевода). Ошибок нет.
pub fn n(d: Decimal) -> String {
    d.normalize().to_string()
}

/// Кнопка шага. Принимает: надпись, номер отрисовки, действие. Ошибок нет.
fn b(text: &str, sid: u64, op: &str) -> Option<serde_json::Value> {
    btn(text, &format!("t:{sid}.{op}"))
}

/// Шаг 1 «Откуда». Принимает: все площадки (с балансами), «откуда» (с USDT > 0), номер, ошибку. Ошибок нет.
pub fn from(all: &[Source], list: &[&Source], sid: u64, err: Option<&str>) -> (Screen, Vec<Ref>) {
    let mut s = Screen::new("Перевести USDT — шаг 1 из 4: откуда?");
    let mut refs = Vec::new();
    for x in list {
        let usdt = x.usdt.map(n).unwrap_or_default();
        s = s.one(b(
            &format!("{} — {usdt} USDT", x.label),
            sid,
            &format!("f{}", refs.len()),
        ));
        refs.push(Ref::End(x.end.clone()));
    }
    for x in all.iter().filter(|x| x.error.is_some()) {
        s = s.line(format!(
            "{}: не прочитано — {}",
            x.label,
            x.error.as_deref().unwrap_or_default()
        ));
    }
    if list.is_empty() {
        s = s.line("Площадок со свободными USDT нет.");
    }
    if let Some(e) = err {
        s = s.line(format!("\n{e}"));
    }
    (s.nav(Some("k:root")), refs)
}

/// Шаг 2 «Куда». Принимает: «откуда» (подпись), список «куда», номер. Ошибок нет.
pub fn to(from: &str, list: &[&Source], sid: u64) -> (Screen, Vec<Ref>) {
    let mut s = Screen::new(format!(
        "Перевести USDT — шаг 2 из 4: куда?\nОткуда: {from}"
    ));
    let mut refs = Vec::new();
    for x in list {
        s = s.one(b(&x.label, sid, &format!("d{}", refs.len())));
        refs.push(Ref::End(x.end.clone()));
    }
    (s.one(b("« Назад", sid, "b")).nav(None), refs)
}

/// Шаг 3 «Сколько». Принимает: подписи концов, сводку «Всё» (или причина, почему перевести нельзя), доли (%), номер,
/// последнюю ошибку ввода. Ошибок нет.
pub fn amount(
    route: (&str, &str),
    q: Result<&Quote, &str>,
    pcts: &[Decimal],
    sid: u64,
    err: Option<&str>,
) -> (Screen, Vec<Ref>) {
    let mut s = Screen::new(format!(
        "Перевести USDT — шаг 3 из 4: сколько?\nС {} на {}",
        route.0, route.1
    ));
    let mut refs = Vec::new();
    match q {
        Err(why) => s = s.line(format!("Перевести нельзя: {why}")),
        Ok(q) => {
            s = s.line(format!(
                "Можно вывести: {} {} (свободно {})",
                n(q.withdrawable),
                q.ticker,
                n(q.available)
            ));
            if let Some(note) = &q.note {
                s = s.line(format!("Удержания: {note}"));
            }
            s = s
                .line(fee_line(q))
                .line(format!("Минимум вывода: {} {}", n(q.min), q.ticker));
            let mut row = vec![b(&format!("Всё ({})", n(q.amount)), sid, "a")];
            for p in pcts {
                row.push(b(&format!("{}%", n(*p)), sid, &format!("p{}", refs.len())));
                refs.push(Ref::Pct(*p));
            }
            s = s.row(row).one(b("Ввести сумму", sid, "e"));
        }
    }
    if let Some(e) = err {
        s = s.line(format!("\n{e}"));
    }
    (s.one(b("« Назад", sid, "b")).nav(None), refs)
}

/// Строка комиссии: у биржи — комиссия вывода в USDT, у кошелька DEX — газ BNB. Ошибок нет.
fn fee_line(q: &Quote) -> String {
    match q.fee {
        Some(f) => format!("Комиссия вывода: {} {}", n(f), q.ticker),
        None => "Комиссия: газ сети BSC в BNB с кошелька".into(),
    }
}

/// Шаг 4 — сводка. Принимает: подписи концов, сводку, номер. Ошибок нет.
pub fn summary(route: (&str, &str), q: &Quote, sid: u64) -> Screen {
    Screen::new(format!(
        "Перевести USDT — шаг 4 из 4: проверьте.\nС {} на {}: {} {}\n{}\nДойдёт: {} {}\nСеть: {} ({})",
        route.0, route.1, n(q.amount), q.ticker, fee_line(q), n(q.arrives), q.ticker, "BSC", q.network
    ))
    .row(vec![b("Отправить", sid, "go"), b("Отмена", sid, "x")])
    .one(b("« Назад", sid, "b"))
    .nav(None)
}
