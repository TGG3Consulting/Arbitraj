//! Файл: core/manual_repeat.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — повторный ОДИНАКОВЫЙ отказ продажи по той же записи «Ручного
//! управления» — не новый алерт, а обновление существующего: тот же id (та же запись на экране и то же сообщение в
//! Telegram — notify/mirror*.rs правит его), причина и счётчик попыток, без нового уведомления stuck.loss_alert.
//! «Тот же отказ» — тот же текст ошибки площадки с точностью до чисел (суммы, проценты, номера блоков меняются от
//! попытки к попытке). Запись, с которой человек запустил продажу, ядро помнит в сделке (Deal.alert_was, manual.rs
//! sell_now). Другой отказ или продажа не с алерта — новый алерт, как раньше.
//! Живой случай 05.11 (сделка 20261105131254836GAIB): каждое нажатие «Продать и перегнать» давало новый алерт и новое
//! уведомление с тем же отказом по квотеру.
//! Кто вызывает: core/manual.rs (raise_alert).
//! От чего зависит: core/types (StuckAlert), deal.rs.
//! Настройки: нет.

use crate::core::types::StuckAlert;

use super::deal::Deal;

/// Прежний алерт сделки, который нужно обновить: продажа запущена с него (Deal.alert_was, забирается) и отказ тот же.
/// Принимает: сделку, текст нового отказа (None — алерт не по отказу продажи). Возвращает: прежний алерт или None.
/// Ошибок нет.
pub fn prior(deal: &mut Deal, error: Option<&str>) -> Option<StuckAlert> {
    let prev = deal.alert_was.take()?;
    let same = match (prev.error.as_deref(), error) {
        (Some(a), Some(b)) => same(a, b),
        _ => false,
    };
    same.then_some(prev)
}

/// Тот же ли отказ: тексты совпадают, если каждую группу цифр (с точками и запятыми внутри) считать одним знаком.
/// Ошибок нет.
pub fn same(a: &str, b: &str) -> bool {
    shape(a) == shape(b)
}

/// Текст без чисел: группа из цифр, точек и запятых, где есть цифра, — «#». Ошибок нет.
fn shape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().any(|c| c.is_ascii_digit()) {
            out.push('#');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' || c == ',' {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Номер попытки обновлённого алерта: прежний + 1 (запись до решения 177 — считается первой). Ошибок нет.
pub fn next_attempt(prev: &StuckAlert) -> u32 {
    prev.attempts.max(1).saturating_add(1)
}

/// Строка хронологии сделки при обновлении записи. Принимает: причину, номер попытки, id алерта. Ошибок нет.
pub fn note(reason: &str, attempts: u32, id: &str) -> String {
    format!(
        "{reason} — тот же отказ, попытка {attempts}: запись {id} обновлена, нового алерта нет — ждём решения человека"
    )
}
