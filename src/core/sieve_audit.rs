//! Файл: core/sieve_audit.rs
//! Что делает: решение 375 и правка ревью t162 (Н3, решение 377) — контроль отсева на живом боте (страховка, НЕ
//! доказательство: доказательство — офлайн-тесты на записанных данных, tests/core_sieve_*.rs). Доля sieve.audit_share
//! показанных отсечённых кандидатов (число 0–1; не задана или 0 — контроля нет) идёт в ОБЫЧНЫЙ полный расчёт (profit.rs
//! decide) на месте самого кандидата — в той же задаче расчёта, поэтому под пределом profit.calc_concurrency и сроком
//! profit.calc_timeout_ms, как любой кандидат (раньше — отдельной задачей tokio без предела); класс Background и метка
//! котировок «audit» (gateway/quote_purpose.rs). Итог — обычным путём кандидата (candidates.rs): отказ — строка и журнал
//! отказа; принят — исполнитель, окно, эпизод, карточка, как любой принятый (сделка одна: задача расчёта направления
//! одна, пока оно в очереди — candidates.rs queued; одна сделка на монету — executor.rs). Принят — ещё и сверка (accepted):
//! граница ПЕРЕСЧИТЫВАЕТСЯ сразу после расчёта (sieve.rs check); снова «отсечь» — нарушение доказательства (запись Error
//! core.sieve_violation и Telegram core.sieve_violation: граница отсекла и до, и после расчёта, а расчёт между ними
//! принял); иначе данные сменились между отсевом и расчётом — запись Info core.sieve_audit, без Telegram. Полный расчёт
//! берёт котировки на ходу (своего снимка всех данных у него нет), поэтому «те же данные» — ближайшее возможное: снимок
//! границы сразу после расчёта; оговорка — в тексте записи. Отбор без случайности: накопитель доли (каждый показанный
//! отсечённый прибавляет долю, на ≥ 1 — контроль и минус 1) — при доле 0,1 контролируется каждый десятый.
//! Кто вызывает: core/sieve_gate.rs (pick), core/candidates.rs (accepted).
//! От чего зависит: sieve.rs (check), sieve_log.rs (счётчики), costs_types.rs (Calc), deps.rs (журнал, уведомления),
//! settings.
//! Настройки: sieve.audit_share.

use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::costs::Calc;
use super::engine::Core;
use super::sieve::Sieve;

/// Накопитель доли.
static ACC: LazyLock<Mutex<Decimal>> = LazyLock::new(|| Mutex::new(Decimal::ZERO));

/// Что делает: идёт ли этот отсечённый кандидат в контрольный полный расчёт (доля sieve.audit_share, накопитель).
/// Принимает: ядро. Возвращает: true — контроль. Ошибок нет.
pub fn pick(core: &Core) -> bool {
    let share = core
        .inner
        .deps
        .settings
        .decimal("sieve.audit_share", At::default())
        .ok()
        .filter(|s| *s > Decimal::ZERO)
        .map(|s| s.min(Decimal::ONE));
    let Some(share) = share else { return false };
    let mut acc = ACC.lock().unwrap_or_else(|e| e.into_inner());
    *acc += share;
    if *acc < Decimal::ONE {
        return false;
    }
    *acc -= Decimal::ONE;
    true
}

/// Что делает: контрольный расчёт ПРИНЯЛ отсечённого кандидата — пересчёт границы сейчас и запись (см. шапку).
/// Сам кандидат дальше идёт обычным путём принятого (вызывающий). Принимает: ядро, направление, расчёт. Ошибок нет.
pub async fn accepted(core: &Core, d: &Direction, calc: &Calc) {
    let now = Box::pin(super::sieve::check(core, d)).await;
    let pair = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
    let nums = format!(
        "прибыль {} USDT, {}%, объём {}",
        calc.profit_usd.round_dp(4),
        calc.profit_pct.round_dp(4),
        calc.qty.normalize()
    );
    let deps = &core.inner.deps;
    let data = serde_json::to_value(calc).ok();
    if let Sieve::Cut { reason, .. } = &now.sieve {
        super::sieve_log::bump("audit_violation");
        let text = format!(
            "{pair}: НАРУШЕНИЕ ДОКАЗАТЕЛЬСТВА ОТСЕВА — граница отсекает кандидата и до, и сразу после полного расчёта, \
             а расчёт его принял ({nums}); граница сейчас: {reason}. Оговорка: полный расчёт берёт котировки на ходу — \
             данные между отсевом и пересчётом могли смениться и вернуться"
        );
        deps.log(
            Level::Error,
            "core.sieve_violation",
            Some(&d.buy_on.0),
            text.clone(),
            data,
        )
        .await;
        deps.notify("core.sieve_violation", Some(&d.buy_on.0), &text)
            .await;
        return;
    }
    super::sieve_log::bump("audit_moved");
    let why = match &now.sieve {
        Sieve::Pass => "граница больше не исключает сделку".to_string(),
        Sieve::Na { class, why } => format!("метод не действует ({class}): {why}"),
        Sieve::Cut { .. } => String::new(),
    };
    let text = format!(
        "{pair}: контроль отсева — кандидат был отсечён, полный расчёт его принял ({nums}); на данных сразу после \
         расчёта {why} — данные сменились, это не нарушение доказательства. Кандидат идёт обычным путём принятого"
    );
    deps.log(
        Level::Info,
        "core.sieve_audit",
        Some(&d.buy_on.0),
        text,
        data,
    )
    .await;
}
