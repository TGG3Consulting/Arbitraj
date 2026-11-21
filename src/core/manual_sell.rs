//! Файл: core/manual_sell.rs
//! Что делает: решение 235 (Тигран, 08.11.2026: «Нажимаю "Продать" — не продаёт, потому что процент минуса больше
//! настройки. Процент минуса меня вообще не волнует, когда я руками нажимаю»). Продажа кнопкой человека («Продать
//! сейчас», «Продать и перегнать» — причина sell::BUTTON) доводится до конца в режиме кнопки — без порогов убытка
//! (stuck.max_loss_pct, stuck.relay_max_loss_pct) и без проверки «выгодно» (режим застрявшей монеты сюда не ходит):
//! (1) отказ до отправки (sell_unsent::decide → Retry::Go: проба откатилась, стакан устарел, нет маршрута …) — снова
//! продажа по рынку кнопкой по реальному балансу (manual_halted::on_hand_sell), предел — тот же
//! stuck.unsent_retries_max (Deal.unsent_retries); исчерпан — алерт с точной причиной (sell_unsent Exhausted);
//! (2) остаток после частичной продажи — снова продажа по рынку кнопкой (свежая котировка; пыль — dust.rs, как всегда);
//! (3) BNB на DEX: газ продажи не помещается сверх резерва — алерт с точной причиной (не режим застрявшей монеты).
//! Защиты исполнения те же, что у любой продажи (trade_ops.rs): худшая цена ордера — по свежему стакану биржи, у DEX —
//! minOut по свежей котировке с допуском exec.dex_slippage_pct. Признак задачи — ручной (risk_count::manual — его
//! ставит запуск кнопки, здесь он не меняется); итог кнопки — одно событие в конце (manual_outcome.rs).
//! Кто вызывает: core/sell.rs (ветки причины sell::BUTTON).
//! От чего зависит: sell.rs (market), sell_unsent.rs (unsent_max), manual_halted.rs (on_hand_sell, set_qty), manual.rs
//! (raise_alert), manual_outcome.rs, trade_ops.rs (убыток для алерта), ledger.rs, deal.rs, op_error.rs.
//! Настройки: stuck.unsent_retries_max (через sell_unsent.rs).

use std::time::Duration;

use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::sell::BUTTON;

/// Повтор продажи кнопки после отказа до отправки: счётчик сделки, строка хронологии и журнал, пауза, затем снова
/// продажа по рынку кнопкой (again). Принимает: ядро, сделку, ошибку, паузу (мс; 0 — без паузы). Ошибок нет.
pub async fn retry(core: &Core, mut deal: Deal, e: &OpError, pause_ms: u64) {
    deal.unsent_retries += 1;
    let max = super::sell_unsent::unsent_max(core);
    let text = format!(
        "кнопка: продажа не ушла: {e} — повтор кнопкой по рынку ({}/{max}), без порогов убытка (решение 235)",
        deal.unsent_retries
    );
    deal.note(text.clone());
    super::ledger::save(core, &deal).await;
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.manual_sell_retry",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    if pause_ms > 0 {
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
    }
    again(core, deal).await
}

/// Остаток после частичной продажи кнопкой — снова по рынку кнопкой. Принимает: ядро, сделку (остаток > 0). Ошибок нет.
pub async fn remainder(core: &Core, mut deal: Deal) {
    deal.note(format!(
        "Продано не всё, остаток {} — кнопка: остаток снова по рынку (решение 235)",
        deal.qty
    ));
    super::ledger::save(core, &deal).await;
    again(core, deal).await
}

/// BNB на DEX: газ продажи кнопкой не помещается сверх резерва — алерт с точной причиной. Принимает: ядро, сделку,
/// ошибку. Ошибок нет.
pub async fn gas(core: &Core, deal: Deal, e: &OpError) {
    let why = format!("продажа не прошла: {e} — газ продажи не помещается сверх резерва BNB");
    alert(core, deal, &why, &e.message).await
}

/// Снова продажа по рынку кнопкой: количество — по реальному балансу площадки (монеты с фонда — на торговый, решение
/// 174), как у первого нажатия. Монет нет или баланс не получен — алерт с причиной. Принимает: ядро, сделку. Ошибок нет.
async fn again(core: &Core, mut deal: Deal) {
    let (at, a) = (deal.at.clone(), deal.direction.asset.clone());
    match super::manual_halted::on_hand_sell(core, &deal, &at, &a, deal.qty).await {
        Ok((qty, note)) => super::manual_halted::set_qty(&mut deal, qty, note),
        Err(e) => {
            let why = format!("повтор продажи кнопкой: {e}");
            return alert(core, deal, &why, &e).await;
        }
    }
    super::ledger::save(core, &deal).await;
    Box::pin(super::sell::market(core, deal, BUTTON)).await
}

/// Алерт человеку (кнопки — manual.rs) и итог кнопки «Не продано: причина». Убыток в алерте — для сведения (по
/// котировке сейчас; нет котировки — неизвестен). Принимает: ядро, сделку, причину, текст отказа (решение 177: тот же
/// отказ — та же запись). Ошибок нет.
async fn alert(core: &Core, deal: Deal, why: &str, error: &str) {
    let loss = super::trade_ops::deal_value(core, &deal)
        .await
        .ok()
        .map(|(v, _)| deal.loss_pct_if(v));
    let d = deal.clone();
    super::manual::raise_alert(core, deal, loss, why.to_string(), Some(error.to_string())).await;
    super::manual_outcome::failed(core, &d, BUTTON, why).await;
}
