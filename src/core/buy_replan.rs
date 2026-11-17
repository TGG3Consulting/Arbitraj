//! Файл: core/buy_replan.rs
//! Что делает: решение 381 — покупка на DEX сорвалась ДО отправки обмена в сеть потому, что цена в пуле ушла
//! («Цена в пуле сейчас X хуже допустимой Y» — exec/order.rs check_price; «свежая средняя цена дороже границы сделки» —
//! предторговая верификация, exec/verify_gate.rs; оба — код QuoteMismatch без факта транзакции), или обмен не попал в
//! блок и его nonce занят нашей отменой в блоке (решение 229: исходная транзакция уже не исполнится, деньги на месте).
//! Было: сделка закрывалась, хотя на свежей цене она могла оставаться выше порога прибыли (живой случай ESIM 17.11:
//! 14 срывов из 17 при плановой прибыли 0.9–4.9 USDT на ~40 USDT). Стало: план считается заново на свежих данных тем же
//! расчётом, что пересчёт перед покупкой (profit.rs decide_capped: сначала объём прошлого плана — быстрый путь решения
//! 266, не прошёл — полный подбор объёма; все затраты, финальная котировка, пороги money.min_profit_usd и
//! money.min_profit_pct) с тем же пределом списания (min(бюджет, свободно)), затем проверка продажи на B (решение 134,
//! precheck_sell.rs). Прибыль выше порогов — покупка по новому плану (новая средняя цена, новая граница сделки); нет —
//! сделка закрывается с причиной обоих отказов. Граница сделки (TradeRequest.worst_price) — как раньше: средняя цена
//! плана × (1 + exec.dex_slippage_pct) (решение 121 B4 не меняется), меняется только план.
//! Стартовое правило Тиграна «ордер отправлен и не исполнился — отменяем, больше не пробуем» не нарушается: здесь
//! обмен в сеть не уходил (или точно не исполнится — 229), это новый план, как у продажи DEX (решение 110,
//! OpError::retry_safe) и «лучшего из двух» (решение 123).
//! Кто вызывает: core/executor.rs (покупка на A).
//! От чего зависит: precheck.rs (replan_sized), precheck_fast.rs (Want), precheck_sell.rs, profit.rs (Verdict),
//! op_error.rs, trade_ops.rs (вид площадки), settings.
//! Настройки: exec.dex_buy_replan_max [площадка DEX покупки] — сколько раз за сделку; не задана или 0 — пересчёта нет.

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;

use super::costs::Calc;
use super::engine::Core;
use super::op_error::OpError;
use super::precheck_fast::Want;
use super::profit::Verdict;

/// Настройка: сколько раз за сделку пересчитывать план покупки DEX после срыва до отправки.
pub const MAX: &str = "exec.dex_buy_replan_max";

/// Что делает: предел пересчётов для площадки покупки. Принимает: ядро, направление. Возвращает: число; площадка
/// покупки — не DEX, настройка не задана или не читается — 0 (пересчёта нет, как до решения 381). Ошибок нет.
pub fn max(core: &Core, d: &Direction) -> u64 {
    if super::trade_ops::kind(core, &d.buy_on) != Ok(VenueKind::Dex) {
        return 0;
    }
    let at = At {
        venue: Some(&d.buy_on.0),
        coin: None,
    };
    core.inner.deps.settings.integer(MAX, at).unwrap_or(0)
}

/// Что делает: можно ли считать план заново после этой ошибки покупки DEX (чистая, тестируется). Да: цена ушла до
/// отправки (QuoteMismatch, транзакции в сети нет — sent = нет) или обмен не попал в блок, а его nonce занят нашей
/// отменой в блоке (решение 229, OpError::cancel_confirmed). Нет: всё остальное (откат в сети, исход неизвестен,
/// отказ канала, нет маршрута, устаревшие данные, настройки). Принимает: ошибку. Ошибок нет.
pub fn replannable(e: &OpError) -> bool {
    (!e.sent && e.code == Some(ErrorCode::QuoteMismatch)) || e.cancel_confirmed()
}

/// Что делает: новый план на свежих данных под предел списания (см. шапку). Принимает: ядро, направление, предел
/// списания (min(бюджет, свободно) — как у прошлой попытки), объём прошлого плана. Возвращает: план в куче (расчёт
/// большой — будущее сделки executor.rs не растёт; выше порогов, списание по худшей цене ≤ предела, продажа на B
/// пройдёт). Ошибки: текст отказа расчёта или проверки продажи.
pub async fn plan(
    core: &Core,
    d: &Direction,
    cap: Decimal,
    qty: Decimal,
) -> Result<Box<Calc>, String> {
    let mut want = Want::new(qty);
    let verdict = Box::pin(super::precheck::replan_sized(
        core,
        d,
        Some(cap),
        Some(&mut want),
    ))
    .await;
    let c = accept(verdict, cap)?;
    Box::pin(super::precheck_sell::check(core, d, c))
        .await
        .map(Box::new)
}

/// Что делает: вердикт расчёта → план (чистая, тестируется). Принимает: вердикт, предел списания. Возвращает: план,
/// если прибыль выше порогов и списание по худшей цене не больше предела. Ошибки: текст отказа.
pub fn accept(v: Verdict, cap: Decimal) -> Result<Calc, String> {
    match v {
        Verdict::Accept(c) if c.spend_max <= cap => Ok(c),
        Verdict::Accept(c) => Err(format!(
            "списание по худшей цене {} больше предела списания {cap}",
            c.spend_max.round_dp(4)
        )),
        Verdict::Reject { reason, .. } => Err(reason),
    }
}

/// Что делает: строка хронологии — план пересчитан. Принимает: номер пересчёта, ошибку попытки, старый и новый план.
/// Возвращает: текст. Ошибок нет.
pub fn accepted_note(n: u64, e: &OpError, old: &Calc, new: &Calc) -> String {
    format!(
        "Пересчёт плана после срыва покупки ({n}): {e} — на свежих данных объём {} вместо {}, средняя цена {} вместо {}, \
         прибыль {} USDT (было {}) выше порогов — покупка по новому плану",
        new.qty,
        old.qty,
        new.buy_quote.avg_price.round_dp(10).normalize(),
        old.buy_quote.avg_price.round_dp(10).normalize(),
        new.profit_usd.round_dp(4),
        old.profit_usd.round_dp(4)
    )
}

/// Что делает: строка хронологии — пересчёт не дал сделки. Принимает: номер пересчёта, причину отказа. Возвращает:
/// текст. Ошибок нет.
pub fn refused_note(n: u64, why: &str) -> String {
    format!("Пересчёт плана после срыва покупки ({n}): на свежих данных сделки нет — {why}")
}
