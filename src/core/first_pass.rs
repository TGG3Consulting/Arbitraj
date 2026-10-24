//! Файл: core/first_pass.rs
//! Что делает: первый проход проверки налогов после запуска. Пока он идёт, вход и выход монет из чёрного списка не
//! уведомляются по каждой монете (иначе при старте — сотни сообщений): в журнал, интерфейс — как обычно, в Telegram —
//! нет; причины копятся. Когда адаптер DEX сообщил о конце первого прохода (MarketEvent::CheckPass), уходит одно
//! сводное coin.check_done: сколько проверено, сколько в чёрном списке, сколько ждут пула или настройки, топ причин.
//! Дальше — посмонетные coin.blacklisted / coin.unblacklisted только на изменения.
//! Кто вызывает: core/rebuild_report.rs (copy причин), core/tick.rs (CheckPass).
//! От чего зависит: engine.rs, deps.rs (notify).
//! Настройки: notify.event.* (coin.check_done), notify.summary_reasons (сколько причин в сводке).

use std::collections::HashMap;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Состояние первого прохода.
#[derive(Debug, Default)]
pub struct FirstPass {
    /// Первый проход закончен (сводка отправлена).
    pub done: bool,
    /// Причины входа в чёрный список за первый проход → сколько монет.
    pub reasons: HashMap<String, usize>,
    /// Сколько монет вошло в чёрный список за первый проход.
    pub blocked: usize,
}

/// Идёт ли первый проход: тогда причина копится, уведомление по монете не нужно. Принимает: причину входа (None —
/// выход из списка). Возвращает: true — первый проход, не уведомлять. Ошибок нет.
pub fn absorb(core: &Core, reason: Option<String>) -> bool {
    let mut f = lk(&core.inner.first_pass);
    if f.done {
        return false;
    }
    if let Some(r) = reason {
        f.blocked += 1;
        *f.reasons.entry(r).or_default() += 1;
    }
    true
}

/// Конец первого прохода у площадки: одно сводное coin.check_done (с топом причин), дальше — посмонетные уведомления.
/// Принимает: площадку и счётчики адаптера. Ошибок нет.
pub async fn done(
    core: &Core,
    venue: &str,
    ok: usize,
    failed: usize,
    waiting: usize,
    config: usize,
) {
    let (top, blocked) = {
        let mut f = lk(&core.inner.first_pass);
        if f.done {
            return;
        }
        f.done = true;
        (std::mem::take(&mut f.reasons), f.blocked)
    };
    let top = top_reasons(core, top);
    let mut text = format!(
        "Первая проверка налогов ({venue}): прошли {ok}, не прошли {failed}, ждут достоверного пула {waiting}, \
         не задана настройка {config}; в чёрном списке {blocked}"
    );
    if !top.is_empty() {
        text.push_str(&format!(". Причины: {top}"));
    }
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.check_done", None, &text, None)
        .await;
    deps.notify("coin.check_done", Some(venue), &text).await;
}

/// Самые частые причины для сводки: notify.summary_reasons штук, по убыванию числа монет. Не задана настройка —
/// пометка вместо причин. Пусто — причин нет. Ошибок нет.
fn top_reasons(core: &Core, reasons: HashMap<String, usize>) -> String {
    if reasons.is_empty() {
        return String::new();
    }
    let n = match core
        .inner
        .deps
        .settings
        .integer("notify.summary_reasons", At::default())
    {
        Ok(n) => n as usize,
        Err(e) => return format!("не показаны — {}", e.message),
    };
    let mut v: Vec<(String, usize)> = reasons.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v.iter()
        .take(n)
        .map(|(r, n)| format!("{n} — {r}"))
        .collect::<Vec<_>>()
        .join("; ")
}
