//! Файл: core/sell_unsent.rs
//! Что делает: продажа не прошла (sell.rs) — повторить её самому или поставить алерт человеку. Повтор — только если
//! продажа не ушла в сеть (денег и газа не стоила) или её исход известен (решение 229), и это не кнопка человека
//! (кроме отказа до отправки — решение 229); общий предел — stuck.unsent_retries_max раз на сделку
//! (Deal.unsent_retries); исчерпан — журнал Error и алерт человеку с кнопками («Продать сейчас», «Беру на себя»,
//! «Продано вручную» — manual.rs, решение 100); у цепочки решения 229 — сначала продажа здесь по порогу
//! (relay_here.rs, решение 220). Всё остальное (ушедшая в сеть и не исполненная продажа без подтверждённой отмены,
//! nonce, нет связи) — как раньше, алерт человеку.
//! Кто вызывает: sell.rs.
//! От чего зависит: op_error.rs, deal.rs, stuck.rs, manual.rs, trade_ops.rs, ledger.rs, deps.rs, relay_here.rs,
//! remain_fact.rs.
//! Настройки: stuck.unsent_retries_max (решение 110), stuck.reject_retry_pause_ms (решение 154); через relay_here.rs —
//! stuck.relay_max_loss_pct, stuck.no_quote_wait_ms (решение 229).
//!
//! Когда повтор:
//! - решение 182: продажа на бирже не исполнилась совсем (OrderNotFilled, монеты на месте) — сразу в режим застрявшей
//!   монеты (свежий стакан); покупки — без изменений (правило «не исполнился — отменяем»);
//! - решение 110: отказ ДО отправки (цена сборки хуже допустимой, лимит или очередь запросов, нет маршрута, данные
//!   устарели, налоги не проверены, откат при пробном вызове — OpError::retry_safe) — сразу в режим застрявшей монеты;
//! - решение 154 (живой случай 04.11, ASTER: «Ни один канал не принял транзакцию», 4 минуты до кнопки человека): все
//!   каналы отправки ответили отказом, и адаптер своим узлом проверил, что транзакции нет ни в блоке, ни в пуле
//!   (OpError::rejected_unsent) — пауза stuck.reject_retry_pause_ms, затем режим застрявшей монеты;
//! - решение 229 (Тигран 08.11: «риска нет: исход известен, деньги на месте»; живой случай JCT 08.11): (1) продажа DEX
//!   не попала в блок за send.max_wait_blocks, а nonce занят нашей отменой в блоке (OpError::cancel_confirmed —
//!   исходная транзакция уже не исполнится) — сразу режим застрявшей монеты; отмена не подтверждена или исход
//!   неизвестен — как раньше, человеку; (2) кнопка человека с отказом ДО отправки (retry_safe: проба откатилась, нет
//!   маршрута …) — тоже повтор, а не сразу человеку (прочие сбои кнопки — как раньше, алерт). Счётчик — тот же. Цепочка,
//!   (Deal.unsent_here — метка случая 229 для хронологии) при исчерпании — один раз продажа здесь по порогу
//!   stuck.relay_max_loss_pct (relay_here::sell_here, решение 220) после сверки остатка с фактом (решение 222); не
//!   вышло — алерт, как раньше. Тигран 08.11: продажа здесь по порогу при исчерпании — у любой цепочки повторов
//!   (110, 154, 182, 229), не только у начатой случаем 229.
//! - решение 234 (Тигран 08.11): повтор продажи кнопки человека — не через режим застрявшей монеты, а снова продажа по
//!   рынку кнопкой (manual_sell.rs: без порогов убытка и «выгодно»); исчерпание — алерт с точной причиной (Exhausted),
//!   продажи здесь по порогу у кнопки нет.
//!
//! Режим застрявшей монеты (stuck.rs) продаёт заново тем же порядком, что и первая продажа: новая котировка
//! (trade_ops::sell_value), новый запрос и новая транзакция с nonce, взятым заново (у адаптера).

use std::time::Duration;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;

/// Что делать с неудачной продажей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    /// Повторить через режим застрявшей монеты после паузы (мс; 0 — без паузы, решение 110).
    Go { pause_ms: u64 },
    /// Повторы исчерпаны: было `max` повторов (stuck.unsent_retries_max) — алерт.
    Exhausted { max: u32 },
    /// Решение 229 (Тигран 08.11 — для любой цепочки): повторы исчерпаны (было `max`) — продажа здесь по порогу
    /// (relay_here.rs).
    Here { max: u32 },
    /// Повтор был бы безопасен, но не настроен — причина (какая настройка не задана); алерт.
    Off(String),
    /// Повтор не положен (ушла в сеть, исход под вопросом, кнопка человека) — алерт как раньше.
    No,
}

/// Решает, повторять ли продажу. Принимает: ядро, сделку, ошибку продажи, причину продажи (кнопка человека —
/// sell::BUTTON — повторяется только при отказе до отправки, решения 110 и 229). Возвращает: Retry. Ошибок нет.
pub fn decide(core: &Core, deal: &Deal, e: &OpError, why: &str) -> Retry {
    if why == super::sell::BUTTON && !e.retry_safe() {
        return Retry::No;
    }
    let pause_ms = if e.retry_safe() || not_filled_cex(core, deal, e) || e.cancel_confirmed() {
        0
    } else if e.rejected_unsent() {
        match setting(core, "stuck.reject_retry_pause_ms") {
            Some(ms) => ms,
            None => {
                return Retry::Off(
                    "повтор продажи после отказа всех каналов не настроен: не задана stuck.reject_retry_pause_ms (решение 154)"
                        .into(),
                );
            }
        }
    } else {
        return Retry::No;
    };
    let max = unsent_max(core);
    if max == 0 {
        return Retry::Off(
            "повторы продажи, не ушедшей в сеть, выключены: stuck.unsent_retries_max не задан или 0".into(),
        );
    }
    if deal.unsent_retries >= max {
        // Решение 229: продажа здесь — один раз (here() поднимает счётчик выше предела), дальше — алерт. Тигран 08.11
        // («продают на месте по порогу убытка»): так у ЛЮБОЙ цепочки повторов (110, 154, 182, 229) — монеты на месте.
        // Решение 234 (Тигран 08.11: «процент минуса меня не волнует, когда я руками нажимаю»): кнопка человека порога
        // убытка не знает — исчерпание её повторов — алерт с точной причиной, без продажи здесь по порогу.
        if deal.unsent_retries == max && why != super::sell::BUTTON {
            return Retry::Here { max };
        }
        return Retry::Exhausted { max };
    }
    Retry::Go { pause_ms }
}

/// Решение 229: случай, с которого цепочка повторов при исчерпании идёт в продажу здесь по порогу — отмена
/// подтверждена (cancel_confirmed) или отказ до отправки после кнопки человека. Принимает: ошибку, причину продажи.
/// Ошибок нет.
pub fn case_229(e: &OpError, why: &str) -> bool {
    e.cancel_confirmed() || (why == super::sell::BUTTON && e.retry_safe())
}

/// Решение 182 (Тигран, 05.11.2026, живой случай GENIUS bsc_dex → gate): продажа на бирже не исполнилась совсем
/// (OrderNotFilled: ордер IOC/FOK по худшей цене снят биржей, монеты на месте, денег не стоило) — монета уже на бирже,
/// бросить её нельзя: повтор через режим застрявшей монеты (свежий стакан, порог убытка застрявшей монеты), как отказ до
/// отправки. Только биржа (у DEX отказ свопа — свои коды). Покупки сюда не ходят — у них прежнее правило «не
/// исполнился — отменяем, не пробуем». Ошибок нет.
fn not_filled_cex(core: &Core, deal: &Deal, e: &OpError) -> bool {
    !e.sent
        && e.code == Some(crate::gateway::errors::ErrorCode::OrderNotFilled)
        && matches!(
            super::trade_ops::kind(core, &deal.at),
            Ok(crate::gateway::contract::VenueKind::Cex)
        )
}

/// Повтор: счётчик сделки, запись в хронологию и журнал, пауза, затем режим застрявшей монеты (новая котировка и
/// новая транзакция — stuck.rs → sell.rs). Принимает: ядро, сделку, ошибку, причину продажи (решение 229 — метка
/// цепочки Deal.unsent_here), паузу (мс). Ошибок нет.
pub async fn retry(core: &Core, mut deal: Deal, e: &OpError, why: &str, pause_ms: u64) {
    deal.unsent_retries += 1;
    deal.unsent_here |= case_229(e, why);
    let max = unsent_max(core);
    let wait = if pause_ms > 0 {
        format!("через {pause_ms} мс ")
    } else {
        String::new()
    };
    // Решение 229: ушла в сеть, но не попала в блок и отменена — «не ушла в сеть» было бы неправдой.
    let what = if e.cancel_confirmed() {
        "продажа не попала в блок, nonce занят отменой (исход известен)"
    } else {
        "продажа не ушла в сеть"
    };
    let text = format!(
        "{what}: {e} — повтор {wait}через режим застрявшей монеты ({}/{max})",
        deal.unsent_retries
    );
    deal.note(text.clone());
    super::ledger::save(core, &deal).await;
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.sell_unsent_retry",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    if pause_ms > 0 {
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
    }
    // Решение 220: причина входа — «не ушла в сеть» или «ордер не исполнился» (решение 182).
    let why = super::stuck_reason::StuckReason::of_sell(e);
    Box::pin(super::stuck::handle(core, deal, why)).await
}

/// Решение 229: повторы цепочки исчерпаны — сверка остатка с фактом (решение 222), затем продажа здесь по порогу
/// stuck.relay_max_loss_pct (relay_here::sell_here: не выше — по рынку, выше / не задан / нет котировки — алерт).
/// Счётчик поднимается выше предела: следующий сбой — алерт (decide → Exhausted). Принимает: ядро, сделку, ошибку,
/// предел. Ошибок нет.
pub async fn here(core: &Core, mut deal: Deal, e: &OpError, max: u32) {
    deal.unsent_retries += 1;
    let text = format!(
        "продажа не прошла: {e} — повторы исчерпаны (stuck.unsent_retries_max = {max}), продажа здесь по порогу {} (решение 229)",
        super::relay_here::SETTING
    );
    deal.note(text.clone());
    super::ledger::save(core, &deal).await;
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.sell_unsent_here",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    if !super::remain_fact::reconcile(core, &mut deal).await {
        return;
    }
    let why = format!("повторы продажи исчерпаны ({max}): {e}");
    Box::pin(super::relay_here::sell_here(core, deal, &why)).await
}

/// Алерт человеку: причина (с пояснением, почему бот сам не повторил), убыток при продаже сейчас, кнопки (manual.rs).
/// Повторы исчерпаны — ещё запись Error в журнал. Принимает: ядро, сделку, ошибку, решение decide. Ошибок нет.
pub async fn alert(core: &Core, deal: Deal, e: &OpError, r: &Retry) {
    let why = match r {
        Retry::Exhausted { max } => {
            let why = format!(
                "продажа не прошла: {e} — не ушла в сеть {} раз подряд, повторы исчерпаны (stuck.unsent_retries_max = {max})",
                deal.unsent_retries + 1
            );
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.sell_unsent_exhausted",
                    Some(&deal.at.0),
                    format!("{}: {why}", deal.id),
                    None,
                )
                .await;
            why
        }
        Retry::Off(off) => format!("продажа не прошла: {e} — {off}"),
        Retry::No | Retry::Go { .. } | Retry::Here { .. } => format!("продажа не прошла: {e}"),
    };
    // Хронологию пишет raise_alert («… — ждём решения человека»): своей строки здесь нет — без дубля (решение 154).
    let value = super::trade_ops::deal_value(core, &deal).await;
    let loss = value.ok().map(|(x, _)| deal.loss_pct_if(x));
    super::manual::raise_alert(core, deal, loss, why, Some(e.message.clone())).await;
}

/// Предел повторов продажи, не ушедшей в сеть (stuck.unsent_retries_max). Не задано — 0: без повторов, сразу алерт.
/// Решение 234: тот же предел у повторов кнопки (manual_sell.rs).
pub(super) fn unsent_max(core: &Core) -> u32 {
    setting(core, "stuck.unsent_retries_max")
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// Целая настройка общей области. Возвращает: None — не задана или не читается. Ошибок нет.
fn setting(core: &Core, name: &str) -> Option<u64> {
    core.inner.deps.settings.integer(name, At::default()).ok()
}
