//! Файл: core/transfer_locked.rs
//! Что делает: повтор вывода после ответа «средства временно заблокированы для вывода» (решение 166, Тигран 05.11.2026).
//! Живые случаи: 04.11 MEXC 16021 «Some pre-crediting assets are temporarily unavailable for withdrawal»; 05.11 KuCoin
//! 115008 «Insufficient withdrawable assets … are currently locked from withdrawal» (сделка 20261105020010981ION) —
//! монета куплена, вывод отклонён, сделка остановлена. Удержание биржа снимает, когда депозит набирает подтверждения
//! сети, поэтому короткий повтор успевает за ним.
//! Правило: код шлюза WithdrawLocked (адаптеры — только по кодам своей документации или живых ответов) и прежний вывод
//! точно не ушёл (OpError::withdraw_unsent, как решение 163) — повтор того же вывода каждые transfer.locked_retry_ms мс,
//! не больше transfer.locked_retries_max раз. Каждая попытка — свой новый ID вывода (адаптер ставит метку времени
//! попытки в withdrawOrderId / remark / requestId / clientId), строка в хронологии сделки «Вывод: попытка N из M —
//! причина» и журнал core.transfer_locked. Успех — сделка идёт дальше обычным путём; все неудачны или настроек нет —
//! как раньше: правило шлюза и остановка (core/transfers.rs) с понятной причиной.
//! Повтор одинаков для всех бирж — и для тех, у кого есть проверка «можно вывести» до покупки
//! (core/precheck_withdraw.rs): он безопасен (вывода не было) и закрывает случай, когда удержание появилось после
//! проверки.
//! Кто вызывает: core/transfers.rs (цикл вывода).
//! От чего зависит: op_error.rs (withdraw_unsent), engine.rs, ledger.rs (сохранение хронологии), withdraw_after.rs (сделка по ID вывода), settings.
//! Настройки: transfer.locked_retry_ms, transfer.locked_retries_max (вся система, можно по площадке-отправителю; без
//! значений по умолчанию: не заданы — повтора нет).

use std::time::Duration;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::op_error::OpError;

/// Повторы по настройкам площадки-отправителя: пауза и сколько повторов всего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub every: Duration,
    pub max: u64,
}

/// Настройки повторов площадки. Возвращает: None — не заданы (или повторов 0). Ошибок нет.
pub fn plan(core: &Core, from: &VenueId) -> Option<Plan> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    let every = s.integer("transfer.locked_retry_ms", at).ok()?;
    let max = s.integer("transfer.locked_retries_max", at).ok()?;
    (max > 0).then(|| Plan {
        every: Duration::from_millis(every),
        max,
    })
}

/// Ошибка — «заблокировано для вывода», и прежний вывод точно не ушёл. Ошибок нет.
pub fn is_locked(e: &GatewayError) -> bool {
    e.code == ErrorCode::WithdrawLocked && OpError::from(e).withdraw_unsent()
}

/// Решение по попытке (чистая функция). Принимает: план (None — не задан), сколько повторов уже сделано. Возвращает:
/// Some(пауза) — повторить; None — повторов нет или исчерпаны. Ошибок нет.
pub fn next(plan: Option<Plan>, done: u64) -> Option<Duration> {
    plan.filter(|p| done < p.max).map(|p| p.every)
}

/// Сбой вывода: повторять ли. Принимает: ядро, сделку, площадку-отправителя, ошибку, сколько повторов уже сделано
/// (увеличивается при повторе). Возвращает: true — пауза выдержана, вызывающий повторяет вывод; false — не
/// WithdrawLocked, повторов нет или они исчерпаны (вызывающий идёт обычным путём правил шлюза). Ошибок нет.
pub async fn again(
    core: &Core,
    deal: &str,
    from: &VenueId,
    e: &GatewayError,
    done: &mut u64,
) -> bool {
    if !is_locked(e) {
        return false;
    }
    let plan = plan(core, from);
    if *done > 0 {
        let of = plan.map(|p| format!(" из {}", p.max)).unwrap_or_default();
        note(
            core,
            deal,
            from,
            &format!("Вывод: попытка {done}{of} — {}", e.message),
        )
        .await;
    }
    let Some(pause) = next(plan, *done) else {
        let why = if plan.is_none() {
            "повтор не настроен (transfer.locked_retry_ms, transfer.locked_retries_max)".to_string()
        } else {
            format!("все {done} повторов неудачны")
        };
        let text = format!(
            "Вывод: {} временно заблокировала средства для вывода — {why}",
            from.0
        );
        note(core, deal, from, &text).await;
        return false;
    };
    if *done == 0 {
        let max = plan.map(|p| p.max).unwrap_or_default();
        let text = format!(
            "Вывод: {} временно заблокировала средства для вывода ({}) — повтор каждые {} мс, не больше {max} раз",
            from.0,
            e.message,
            pause.as_millis()
        );
        note(core, deal, from, &text).await;
    }
    tokio::time::sleep(pause).await;
    *done += 1;
    true
}

/// Строка в хронологию сделки (и сохранить) и журнал core.transfer_locked. Сделки нет — только журнал. Ошибок нет.
/// t89: у возврата USDT — в закрытую сделку (ID вывода = сделка + метка попытки).
async fn note(core: &Core, deal: &str, from: &VenueId, text: &str) {
    // Решение 253, дополнение t89: у возврата USDT ID вывода — сделка + метка попытки (withdraw_after::deal_of).
    let key = super::withdraw_after::deal_of(core, deal);
    let d = {
        let mut deals = lk(&core.inner.deals);
        key.and_then(|k| deals.get_mut(&k)).map(|d| {
            d.note(text.to_string());
            d.clone()
        })
    };
    if let Some(d) = d {
        super::ledger::save(core, &d).await;
    }
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.transfer_locked",
            Some(&from.0),
            format!("Сделка {deal}: {text}"),
            None,
        )
        .await;
}
