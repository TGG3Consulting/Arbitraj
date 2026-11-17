//! Файл: core/buy_replan.rs
//! Что делает: решение 381 — покупка на DEX сорвалась ДО отправки обмена в сеть, потому что цена в пуле ушла:
//! «Цена в пуле сейчас X хуже допустимой Y» (exec/order.rs check_price) или предторговая верификация «свежая средняя
//! цена дороже границы сделки» (exec/verify_gate.rs; не грубый предохранитель дрейфа). Признак — пометка адаптера
//! GatewayError::price_moved → OpError::price_moved (ревью t167, Н2: по признаку, не по тексту); прочие QuoteMismatch
//! (сверка с квотером, минимум 0, новый approve, «обе хуже» лучшего из двух) пересчёта не получают.
//! Было: сделка закрывалась, хотя на свежей цене она могла оставаться выше порога прибыли (живой случай ESIM 17.11:
//! 14 срывов из 17 при плановой прибыли 0.9–4.9 USDT на ~40 USDT, 2.3–12 % вложения). Стало: план считается заново на
//! свежих данных тем же расчётом, что пересчёт перед покупкой (profit.rs decide_capped: сначала объём прошлого плана —
//! быстрый путь решения 266, не прошёл — полный подбор объёма; все затраты, финальная котировка, пороги
//! money.min_profit_usd и money.min_profit_pct) с тем же пределом списания (min(бюджет, свободно)), затем проверка
//! продажи на B (решение 134, precheck_sell.rs) и газа BNB на DEX-сторонах (решение 62, как в precheck.rs: первая
//! попытка могла потратить BNB на approve — ревью t167, Н1). Перед повторной покупкой — те же замки, что перед первой
//! (ревью t167, Б1): risk::can_open (пауза площадки, правило шлюза StopVenue, «Стоп»/«Пауза» человека) и замок сухого
//! режима (executor.rs buy_gate) — ready ниже. Граница сделки (TradeRequest.worst_price) — как раньше: средняя цена
//! плана × (1 + exec.dex_slippage_pct) (решение 121 B4 не меняется), меняется только план.
//! Стартовое правило Тиграна «ордер отправлен и не исполнился — отменяем, больше не пробуем» не нарушается: обмен в сеть
//! не уходил (sent = нет), это новый план, как у продажи DEX (решение 110) и «лучшего из двух» (решение 123).
//! «Не попала в блок, nonce занят нашей отменой» (решение 229 — только продажа) сюда НЕ входит: обмен был отправлен;
//! вопрос владельцу — docs/TASKS.md (ревью t167, Б2).
//! Кто вызывает: core/executor.rs (покупка на A).
//! От чего зависит: precheck.rs (replan_sized, bnb_ok, gas_need), precheck_fast.rs (Want), precheck_sell.rs,
//! profit.rs (Verdict), op_error.rs, trade_ops.rs (вид площадки), costs_types.rs, notify/human, settings.
//! Настройки: exec.dex_buy_replan_max [площадка DEX покупки] — сколько раз за сделку; не задана или 0 — пересчёта нет;
//! через вызываемые — всё, что читают profit.rs и precheck.rs (risk.min_bnb_balance).
//! Уведомление: deal.replanned (план покупки пересчитан и принят).

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

/// Событие уведомлений: план покупки пересчитан и принят.
pub const EVENT: &str = "deal.replanned";

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

/// Что делает: можно ли считать план заново после этой ошибки покупки DEX (чистая, тестируется). Да — только «цена
/// ушла» до отправки: QuoteMismatch с пометкой адаптера price_moved и без транзакции в сети. Нет — всё остальное
/// (другие QuoteMismatch, откат в сети, «не попала в блок» с отменой или без, исход неизвестен, отказ канала, нет
/// маршрута, устаревшие данные, настройки). Принимает: ошибку. Ошибок нет.
pub fn replannable(e: &OpError) -> bool {
    !e.sent && e.price_moved && e.code == Some(ErrorCode::QuoteMismatch)
}

/// Что делает: решение перед повторной покупкой (ревью t167, Б1; чистая, тестируется): обе проверки, что перед первой
/// покупкой, — risk::can_open (паузы и «Стоп») и замок сухого режима (executor.rs buy_gate), в этом порядке. Принимает:
/// их итоги. Возвращает: Ok — покупать; Err — текст причины закрытия сделки. Ошибок нет (всё в Err).
pub fn ready(open: Result<(), String>, gate: Result<(), String>) -> Result<(), String> {
    open.and(gate)
        .map_err(|why| format!("перед покупкой по пересчитанному плану: {why} — ордер не ставился"))
}

/// Что делает: новый план на свежих данных под предел списания (см. шапку). Принимает: ядро, направление, предел
/// списания (min(бюджет, свободно) — как у прошлой попытки), объём прошлого плана. Возвращает: план в куче (расчёт
/// большой — будущее сделки executor.rs не растёт; выше порогов, списание по худшей цене ≤ предела, продажа на B
/// пройдёт, BNB на газ хватает). Ошибки: текст отказа расчёта, проверки продажи или газа.
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
    let c = Box::pin(super::precheck_sell::check(core, d, c)).await?;
    gas_ok(core, d, &c).await?;
    Ok(Box::new(c))
}

/// Что делает: решение 62 (ревью t167, Н1) — газ всех транзакций сделки на DEX-сторонах сверх резерва по НОВОМУ плану,
/// та же проверка и те же этапы, что в предпроверке (precheck.rs: A — покупка и перевод, B — продажа и возврат USDT);
/// мало — уведомление risk.low_bnb, как там. Принимает: ядро, направление, план. Ошибки: текст причины.
async fn gas_ok(core: &Core, d: &Direction, c: &Calc) -> Result<(), String> {
    for (v, stages) in [
        (&d.buy_on, ["A", "перевод"]),
        (&d.sell_on, ["B", super::costs_types::RETURN_STAGE]),
    ] {
        if super::trade_ops::kind(core, v)? != VenueKind::Dex {
            continue;
        }
        let need = super::precheck::gas_need(&c.costs, &stages);
        if let Err(e) = super::precheck::bnb_ok(core, d, v, need).await {
            core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
            return Err(e);
        }
    }
    Ok(())
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

/// Что делает: уведомление deal.replanned (ревью t167, Н4) — план покупки пересчитан и принят: монета, площадки,
/// новый объём и прибыль против прошлых, причина срыва. Очередь уведомлений сделку не ждёт (решение 340). Принимает:
/// ядро, направление, id сделки, ошибку попытки, старый и новый план. Ошибок нет.
pub async fn notify(core: &Core, d: &Direction, deal: &str, e: &OpError, old: &Calc, new: &Calc) {
    use crate::notify::human::{self, Msg};
    let text = Msg::new(human::INFO, "План покупки пересчитан: цена ушла")
        .line(format!(
            "{} {} → {}",
            d.asset.ticker,
            human::venue(&d.buy_on.0),
            human::venue(&d.sell_on.0)
        ))
        .kv(
            "Объём",
            format!("{} (было {})", human::qty(new.qty), human::qty(old.qty)),
        )
        .kv(
            "План",
            format!(
                "{} (было {})",
                human::signed_usd(new.profit_usd),
                human::signed_usd(old.profit_usd)
            ),
        )
        .line(e.to_string())
        .id(deal)
        .render();
    core.inner
        .deps
        .notify(EVENT, Some(&d.buy_on.0), &text)
        .await;
}
