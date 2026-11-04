//! Файл: core/usdt_return_deal.rs
//! Что делает: общий шаг «после закрытия — возврат USDT на площадку покупки» (решение 138). Вызывается из ОДНОГО места —
//! ledger::finish при Step::Closed, поэтому ни один путь закрытия (обычная продажа, застрявшая монета и «Перегон» —
//! продано здесь или на третьей площадке, «Продать сейчас», «Продать и перегнать», «Вернуть боту», «Продано вручную»,
//! пересчёт до покупки) не может его пропустить. Возврат нужен — режим usdt_return.mode = per_trade или флаг сделки
//! return_usdt («Продать и перегнать», решение 101) — и выручка не на площадке покупки (deal.at ≠ buy_on).
//! Перегон — отдельная задача класса Trade (путь сделки, решение 61: раньше задача шла фоном — классом Background в
//! очереди узла BSC); итог — в состояние сделки (usdt_return_state.rs). Возврата нет и не будет (сбой, нет кошелька,
//! задача оборвалась, исключённый путь — Deal.usdt_return_skip) — страховка «USDT не вернулись».
//! Кто вызывает: core/ledger.rs (finish).
//! От чего зависит: usdt_return_job.rs (запуск перегона), usdt_return_state.rs, dex_account.rs, deal.rs, settings.
//! Настройки: usdt_return.mode, venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::usdt_return_state::{self as st, UsdtReturn};

pub use super::usdt_return_job::Job;

/// Нужен ли возврат этой закрытой сделке: per_trade или «Продать и перегнать». Ошибок нет: режим не задан — нет.
pub fn wanted(core: &Core, deal: &Deal) -> bool {
    let mode = core
        .inner
        .deps
        .settings
        .text("usdt_return.mode", At::default());
    // Варианты настройки — per_trade / separate (settings/catalog/trade.rs).
    deal.return_usdt || mode.ok().as_deref() == Some("per_trade")
}

/// Решает возврат закрытой сделки и ставит состояние в сделку (сохранит ledger::finish). Принимает: ядро, сделку.
/// Возвращает: Job — запустить после сохранения; None — возврат не нужен или уже решён. Ошибок нет: сбой — страховка.
pub async fn prepare(core: &Core, deal: &mut Deal) -> Option<Job> {
    if deal.usdt_return.is_some() || !wanted(core, deal) {
        return None;
    }
    let deps = &core.inner.deps;
    if deal.at == deal.direction.buy_on {
        // Решение 108: продано там же, где куплено (монета не уезжала) — выручка уже на площадке покупки.
        if deal.return_usdt {
            let text = format!(
                "Сделка {}: перегон не нужен — продано на {}, выручка уже на площадке покупки",
                deal.id, deal.at.0
            );
            deps.log(
                Level::Info,
                "core.usdt_return",
                Some(&deal.at.0),
                text,
                None,
            )
            .await;
        }
        return None;
    }
    // Решение 118 (Я-22): уходит то, что пришло: выручка минус комиссия продажи в котируемой монете площадки.
    let quote = deps.settings.text("venue.quote_asset", at(&deal.at));
    let amount = match &quote {
        Ok(q) => deal.proceeds - sell_fee(deal, q),
        Err(_) => deal.proceeds,
    };
    if amount <= Decimal::ZERO {
        return None;
    }
    let from = deal.at.clone();
    // Исключённый путь (например «Продано вручную», решение 100) — возврата не будет: страховка.
    if let Some(why) = deal.usdt_return_skip.clone() {
        return not_returned(core, deal, amount, &from, &why).await;
    }
    // Решение 132: перегон выручки — кошельком сделки.
    let acc = match super::dex_account::of(core, deal) {
        Ok(a) => a,
        Err(e) => {
            return not_returned(core, deal, amount, &from, &format!("перегон не начат: {e}"))
                .await;
        }
    };
    let to = deal.direction.buy_on.clone();
    let text = format!(
        "Сделка {}: возврат USDT {amount}: {} → {} начат",
        deal.id, from.0, to.0
    );
    deal.note(text.clone());
    deps.log(Level::Info, "core.usdt_return", Some(&from.0), text, None)
        .await;
    deal.usdt_return = Some(UsdtReturn::Pending {
        amount,
        from: from.clone(),
        to: to.clone(),
    });
    Some(Job {
        deal: deal.id.clone(),
        amount,
        from,
        to,
        acc,
    })
}

/// Возврата не будет: состояние в сделку, страховка. Возвращает: None (перегона нет). Ошибок нет.
async fn not_returned(
    core: &Core,
    deal: &mut Deal,
    amount: Decimal,
    from: &VenueId,
    why: &str,
) -> Option<Job> {
    deal.note(st::not_returned_text(&deal.id, amount, from, why));
    deal.usdt_return = Some(UsdtReturn::NotReturned {
        amount,
        at: from.clone(),
        why: why.to_string(),
    });
    st::alarm(core, &deal.id, amount, from, why).await;
    None
}

/// Комиссия продаж сделки в котируемой монете (её нет в выручке). Принимает: сделку, тикер котируемой. Ошибок нет.
fn sell_fee(deal: &Deal, quote: &str) -> Decimal {
    deal.sells
        .iter()
        .filter(|r| r.fee_asset == quote)
        .map(|r| r.fee)
        .sum()
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At. Ошибок нет.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
