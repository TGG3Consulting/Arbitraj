//! Файл: core/sieve_audit.rs
//! Что делает: решение 364 — контроль отсева на живом боте (страховка, НЕ доказательство: доказательство — офлайн-тесты
//! на записанных данных, tests/core_sieve_t162.rs). Доля sieve.audit_share отсечённых кандидатов (число 0–1; не задана
//! или 0 — контроля нет) всё равно считается полным расчётом (profit.rs decide) в фоне — классом Background и с меткой
//! котировок «audit» (gateway/quote_purpose.rs), очередь кандидатов его не ждёт. Если полный расчёт ПРИНЯЛ бы
//! отсечённого кандидата — это нарушение доказательства (должно быть ноль): запись Error core.sieve_violation с
//! расчётом и уведомление Telegram core.sieve_violation. Отбор без случайности: накопитель доли (каждый отсечённый
//! прибавляет долю, на ≥ 1 — проверка и минус 1) — при доле 0,1 проверяется каждый десятый.
//! Кто вызывает: core/sieve_log.rs (cut).
//! От чего зависит: profit.rs (decide), dry.rs (режим), gateway/urgency.rs, gateway/quote_purpose.rs, deps.rs (журнал,
//! уведомления), settings.
//! Настройки: sieve.audit_share.

use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::quote_purpose::{self, Purpose};
use crate::gateway::urgency::{self, Urgency};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::profit::Verdict;

/// Накопитель доли.
static ACC: LazyLock<Mutex<Decimal>> = LazyLock::new(|| Mutex::new(Decimal::ZERO));

/// Что делает: по доле sieve.audit_share ставит фоновый полный расчёт отсечённого кандидата. Принимает: ядро,
/// направление. Ошибок нет.
pub fn maybe(core: &Core, d: &Direction) {
    let share = core
        .inner
        .deps
        .settings
        .decimal("sieve.audit_share", At::default())
        .ok()
        .filter(|s| *s > Decimal::ZERO)
        .map(|s| s.min(Decimal::ONE));
    let Some(share) = share else { return };
    {
        let mut acc = ACC.lock().unwrap_or_else(|e| e.into_inner());
        *acc += share;
        if *acc < Decimal::ONE {
            return;
        }
        *acc -= Decimal::ONE;
    }
    let (core, d) = (core.clone(), d.clone());
    tokio::spawn(async move {
        let mode = super::dry::mode(&core.inner.deps.settings);
        let work = urgency::scope(
            Urgency::Background,
            quote_purpose::scope(Purpose::Audit, super::profit::decide(&core, &d, &mode)),
        );
        if let Verdict::Accept(calc) = Box::pin(work).await {
            let text = format!(
                "{} {} → {}: НАРУШЕНИЕ ДОКАЗАТЕЛЬСТВА ОТСЕВА — кандидат отсечён границей, а полный расчёт его принял \
                 (прибыль {} USDT, {}%, объём {})",
                d.asset.ticker,
                d.buy_on.0,
                d.sell_on.0,
                calc.profit_usd.round_dp(4),
                calc.profit_pct.round_dp(4),
                calc.qty.normalize()
            );
            let deps = &core.inner.deps;
            let data = serde_json::to_value(&calc).ok();
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
        }
    });
}
