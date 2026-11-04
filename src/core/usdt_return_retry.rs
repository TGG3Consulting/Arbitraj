//! Файл: core/usdt_return_retry.rs
//! Что делает: повтор возврата выручки USDT закрытой сделки на площадку покупки (решение 163, Тигран 05.11.2026).
//! Живой случай: DOGE bingx → mexc закрыта с прибылью, возврат 83.10 USDT mexc → bingx отказан MEXC 16021
//! («pre-crediting assets temporarily unavailable for withdrawal») — «не вернулись», бот больше не пытался.
//! Теперь: перегон не состоялся, а прежний вывод точно не ушёл (retry_safe: площадка отказала или сбой до вывода) —
//! повтор каждые usdt.return_retry_s секунд, не больше usdt.return_retries_max повторов; сумма каждого повтора —
//! свободные USDT площадки, не больше выручки сделки. Каждая попытка — журнал (core.usdt_return) и хронология сделки
//! «Возврат USDT: попытка N из M — причина». Все неудачны, исход неизвестен или настроек нет — «не вернулись»:
//! страховка (usdt.not_returned) и запись с кнопками на экране «Ручное управление» (core/manual_usdt.rs).
//! Состояние — в учёте сделки (UsdtReturn::Retrying): после перезапуска повторы продолжаются (resume). Во время
//! попытки состояние «идёт» (Pending): перезапуск посреди попытки — «не вернулись» (перевод мог уйти, повтора нет).
//! Кто вызывает: usdt_return_job.rs (сбой первой попытки), engine.rs (resume при запуске), manual_usdt.rs.
//! От чего зависит: usdt_return_attempt.rs (попытка, захват, хронология), usdt_return_state.rs, gateway/urgency.rs,
//! settings.
//! Настройки: usdt.return_retry_s, usdt.return_retries_max (по площадке, где лежат USDT; без значений по умолчанию:
//! не заданы — без повторов, как до решения 163).

use std::time::Duration;

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::urgency::{self, Urgency};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::op_error::OpError;
use super::usdt_return_attempt::{attempt, claim, log, note, retrying};
use super::usdt_return_state::{self as st, UsdtReturn};

/// Повторы по настройкам площадки: пауза и сколько повторов всего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub every: Duration,
    pub max: u64,
}

/// Настройки повторов площадки, где лежат USDT. Возвращает: None — не заданы (или повторов 0). Ошибок нет.
pub fn plan(core: &Core, from: &VenueId) -> Option<Plan> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    let every = s.integer("usdt.return_retry_s", at).ok()?;
    let max = s.integer("usdt.return_retries_max", at).ok()?;
    (max > 0).then(|| Plan {
        every: Duration::from_secs(every),
        max,
    })
}

/// Прежний вывод точно не ушёл — повтор не создаст двойного вывода (OpError::withdraw_unsent). Ошибок нет.
pub fn retry_safe(e: &OpError) -> bool {
    e.withdraw_unsent()
}

/// Сбой попытки возврата: решает, повторять ли. Принимает: ядро, сделку, выручку (предел суммы), откуда, куда,
/// сколько повторов уже сделано (0 — сбой первой попытки при закрытии), ошибку. Возвращает: true — состояние
/// «ждёт повтора» (вызывающий запускает или продолжает повторы); false — «не вернулись» со страховкой. Ошибок нет.
pub async fn fail(
    core: &Core,
    deal: &str,
    amount: Decimal,
    (from, to): (&VenueId, &VenueId),
    tries: u64,
    e: &OpError,
) -> bool {
    let plan = plan(core, from);
    if tries > 0 {
        let of = plan.map(|p| format!(" из {}", p.max)).unwrap_or_default();
        note(
            core,
            deal,
            &format!("Возврат USDT: попытка {tries}{of} — {e}"),
        )
        .await;
    }
    let why = if e.outcome_unknown() {
        format!("исход перегона неизвестен ({e}) — проверьте обе площадки, перевод мог уйти")
    } else if !retry_safe(e) {
        format!(
            "перегон не состоялся: {e} — вывод мог уйти (нет ответа или он не разобран), повтора нет — проверьте обе площадки"
        )
    } else if let Some(p) = plan.filter(|p| tries < p.max) {
        let text = match tries {
            0 => format!(
                "Возврат USDT не состоялся: {e} — повтор через {} с, повторов не больше {}",
                p.every.as_secs(),
                p.max
            ),
            n => format!(
                "повтор {} из {} через {} с",
                n + 1,
                p.max,
                p.every.as_secs()
            ),
        };
        let state = UsdtReturn::Retrying {
            amount,
            from: from.clone(),
            to: to.clone(),
            tries,
            why: e.to_string(),
        };
        // Повтор > 0: строка «попытка N из M — причина» уже в хронологии и журнале (note).
        let shown = if tries == 0 { text.as_str() } else { "" };
        st::set(core, deal, state, shown).await;
        if tries == 0 {
            log(core, Level::Warn, from, format!("Сделка {deal}: {text}")).await;
        }
        return true;
    } else if let Some(p) = plan {
        format!("все {} повторов не удались, последняя причина: {e}", p.max)
    } else {
        if tries == 0 {
            let t = format!(
                "Сделка {deal}: повторы возврата USDT не заданы (usdt.return_retry_s, usdt.return_retries_max) — без повторов"
            );
            log(core, Level::Info, from, t).await;
        }
        format!("перегон не состоялся: {e}")
    };
    st::mark_not_returned(core, deal, amount, from, &why).await;
    false
}

/// Запускает повторы отдельной задачей класса Trade (путь сделки). Принимает: ядро, сделку. Ошибок нет.
pub fn schedule(core: &Core, deal: &str) {
    let (c, d) = (core.clone(), deal.to_string());
    tokio::spawn(urgency::scope(
        Urgency::Trade,
        async move { run(&c, &d).await },
    ));
}

/// После перезапуска: сделки «ждёт повтора» продолжают повторы с того же счёта. Ошибок нет.
pub fn resume(core: &Core) {
    let ids: Vec<String> = lk(&core.inner.deals)
        .values()
        .filter(|d| matches!(d.usdt_return, Some(UsdtReturn::Retrying { .. })))
        .map(|d| d.id.clone())
        .collect();
    for id in ids {
        schedule(core, &id);
    }
}

/// Повторы: пауза, попытка, итог — пока состояние «ждёт повтора» и повторы не кончились. Ошибок нет.
async fn run(core: &Core, deal: &str) {
    loop {
        let Some((amount, from, to, tries, last)) = retrying(core, deal) else {
            return;
        };
        // Настройки сменились на лету: повторы выключены или их предел уже исчерпан — «не вернулись».
        let Some(p) = plan(core, &from).filter(|p| tries < p.max) else {
            let why = match plan(core, &from) {
                Some(p) => format!(
                    "все {} повторов не удались, последняя причина: {last}",
                    p.max
                ),
                None => format!(
                    "повторы выключены (usdt.return_retry_s, usdt.return_retries_max не заданы), последняя причина: {last}"
                ),
            };
            return st::mark_not_returned(core, deal, amount, &from, &why).await;
        };
        tokio::time::sleep(p.every).await;
        let n = tries + 1;
        // Захват попытки: «ждёт повтора» с тем же счётом → «идёт». Вторая задача по той же сделке не пройдёт.
        if !claim(core, deal, tries).await {
            return;
        }
        match attempt(core, deal, amount, (&from, &to), &format!("u{n}")).await {
            Ok((sent, credited)) => {
                let t = format!(
                    "USDT вернулись на попытке {n} из {}: {sent} {} → {}, зачислено {credited}",
                    p.max, from.0, to.0
                );
                log(core, Level::Info, &from, format!("Сделка {deal}: {t}")).await;
                let state = UsdtReturn::Done {
                    amount: sent,
                    credited,
                    from,
                    to,
                };
                return st::set(core, deal, state, &t).await;
            }
            Err(e) => {
                if !fail(core, deal, amount, (&from, &to), n, &e).await {
                    return;
                }
            }
        }
    }
}
