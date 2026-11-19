//! Файл: core/remain_apply.rs
//! Что делает: решение 222, 405, 413 (вынесено из remain_fact.rs — размер файла, седьмое ревью t198) — применение
//! факта счёта к сделке: своя ждёт доли — подтверждается долей распределения (credit_apply::own); подтверждённая —
//! min(учёт, факт − других, с долями ждущих), всё чужое — алерт человеку (remain_alert::all_foreign); на счёте ничего —
//! фантом закрывается (остаток «не присвоено» обнуляется — шестое ревью, Б-1). Текст расхождения — что произошло,
//! какая сделка, сколько по учёту / на счёте / у других (требование Тиграна 18.11), журнал core.remain_fact.
//! Кто вызывает: core/remain_fact.rs (reconcile_as, apply_fact).
//! От чего зависит: credit_split.rs, credit_apply.rs, credit_fact.rs, remain_others.rs, remain_alert.rs,
//! manual_halted.rs (set_qty), ledger.rs, deal_words.rs, deps.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Что делает: как apply_fact, с учётом других, снятым вместе с фактом (reconcile). Решение 413: распределение факта
/// между ждущими долю (credit_split.rs): своя ждёт — подтверждается долей (credit_apply::own); иначе ждущие другие
/// вычитаются своими долями, не записями. Принимает: ядро, сделку, факт, других. Возвращает: как reconcile.
/// Ошибок нет.
pub async fn apply_with(
    core: &Core,
    deal: &mut Deal,
    fact: Decimal,
    others: super::remain_others::Others,
) -> bool {
    let sp = super::credit_split::split(&super::credit_apply::input(core, deal, fact, &others));
    if super::credit_fact::pending(deal) {
        return Box::pin(super::credit_apply::own(core, deal, fact, &others, &sp)).await;
    }
    let others = super::credit_apply::with_shares(&others, &sp);
    let qty = match super::remain_others::own_qty(deal.qty, fact, &others) {
        super::remain_others::Own::Qty(q) => q,
        super::remain_others::Own::Human => {
            return Box::pin(super::remain_alert::all_foreign(core, deal, fact, &others)).await;
        }
    };
    if qty == deal.qty {
        return true;
    }
    // Требование Тиграна 18.11: что произошло, какая сделка, сколько по учёту / на счёте / у других, что делать.
    let t = &deal.direction.asset.ticker;
    let mut text = format!(
        "{}: по учёту у сделки {}, а на счёте {} сейчас {}",
        super::deal_words::deal(core, deal),
        super::deal_words::coins(deal.qty, t),
        super::deal_words::place(core, &deal.at, deal.dex_account.as_ref()),
        super::deal_words::coins(fact, t)
    );
    if others.qty > Decimal::ZERO {
        text.push_str(&format!(
            ", из них {} принадлежат другим сделкам: {}",
            super::deal_words::coins(others.qty, t),
            others.words.join("; ")
        ));
    } else if !others.words.is_empty() {
        text.push_str(&format!(
            " (через этот счёт идёт: {})",
            others.words.join("; ")
        ));
    }
    text.push_str(&format!(
        ". Бот продаёт только монеты этой сделки: {}; не хватает {} — проверьте баланс {t} на {} и хронологию \
         сделки {} (решение 222, 405)",
        super::deal_words::coins(qty, t),
        super::deal_words::coins(deal.qty - qty, t),
        super::deal_words::place(core, &deal.at, deal.dex_account.as_ref()),
        deal.id
    ));
    log(core, deal, &text).await;
    super::manual_halted::set_qty(deal, qty, None);
    if qty > Decimal::ZERO {
        super::ledger::save(core, deal).await;
        return true;
    }
    // Шестое ревью (Б-1): монет на счёте нет — и остатку взяться неоткуда: фантом закрывается, как раньше.
    super::credit_fact::set_rest(deal, Decimal::ZERO);
    let why = "остатка на площадке нет — фантом учёта (решение 222)";
    Box::pin(super::ledger::finish(core, deal, Step::Closed, why)).await;
    false
}

/// Запись в журнал core.remain_fact. Ошибок нет.
pub async fn log(core: &Core, deal: &Deal, text: &str) {
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.remain_fact",
            Some(&deal.at.0),
            text,
            None,
        )
        .await;
}
