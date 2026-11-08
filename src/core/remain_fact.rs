//! Файл: core/remain_fact.rs
//! Что делает: решение 222 — страховка от «фантомного остатка» на ВСЕХ площадках. Перед тем как автоматика ведёт
//! остаток сделки (режим застрявшей монеты stuck::handle: «Ожидание», «Порог убытка», «Перегон», продажа по рынку,
//! повторы неушедшей продажи — sell_unsent.rs, остаток после частичной продажи — sell.rs) учёт сделки сверяется с
//! фактическим балансом площадки, и дальше работает min(учёт, факт) — так же, как ручная команда «Продать сейчас»
//! (manual_halted.rs: «На площадке X из Y — продаётся то, что есть», та же set_qty). Живой случай 08.11 (сделка
//! 20261108094918374PIT): по учёту остаток 12162067866.1632 PIT, на кошельке — 19259.884524163; продажи и перегон
//! такого остатка откатывались (token call revert, SafeMath: subtraction overflow).
//! Факт: DEX — баланс кошелька сделки (deal.dex_account, решение 132; manual_halted::free_of); биржа — монета на
//! торговом счёте и на счетах, с которых она перекладывается на торговый (роль ToTrading: фонд и т.п., решение 174),
//! free + locked — только чтение, без переводов: монета, не переложенная на торговый, — не фантом.
//! Больше учёта не берётся никогда: монета той же площадки может принадлежать другой сделке (остановленной или «Беру
//! на себя») — её монеты могут скрыть фантом этой сделки, но в эту сделку не попадут.
//! Факта нет (баланс не прочитан, счёт с ошибкой) — учёт как раньше, строка в хронологии и журнал core.remain_fact.
//! На площадке ничего нет — сделка закрывается (остатка нет; возврат USDT — общий шаг закрытия ledger::finish).
//! Нативная BNB на DEX (решение 62) не сверяется: её баланс — общий с резервом на газ (native.rs, отдельные правила).
//! Кто вызывает: core/stuck.rs (handle).
//! От чего зависит: manual_halted.rs (free_of, set_qty), dex_account.rs, venue_ticker.rs, native.rs, ledger.rs, deal.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::gateway::contract::{AccountRole, VenueKind};
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Сколько вести дальше: min(учёт, факт), не меньше нуля. Принимает: учёт сделки, факт площадки. Ошибок нет.
pub fn settle_qty(account: Decimal, fact: Decimal) -> Decimal {
    account.min(fact).max(Decimal::ZERO)
}

/// Сверка остатка сделки с фактом площадки (решение 222). Принимает: ядро, сделку (количество меняется). Возвращает:
/// true — вести дальше; false — на площадке ничего нет, сделка закрыта. Ошибок нет: факта нет — учёт как раньше.
pub async fn reconcile(core: &Core, deal: &mut Deal) -> bool {
    if deal.qty <= Decimal::ZERO || super::native::is_native(core, &deal.direction.asset) {
        return true;
    }
    let fact = match fact(core, deal).await {
        Ok(f) => f,
        Err(e) => {
            let text =
                format!("Сверка остатка с балансом площадки не вышла: {e} — по учёту сделки");
            log(core, deal, &text).await;
            deal.note(text);
            return true;
        }
    };
    let qty = settle_qty(deal.qty, fact);
    if qty == deal.qty {
        return true;
    }
    let text = format!(
        "{}: по учёту {} {}, на {} — {fact}; дальше {qty} (решение 222)",
        deal.id, deal.qty, deal.direction.asset.ticker, deal.at.0
    );
    log(core, deal, &text).await;
    super::manual_halted::set_qty(deal, qty, None);
    if qty > Decimal::ZERO {
        super::ledger::save(core, deal).await;
        return true;
    }
    let why = "остатка на площадке нет — фантом учёта (решение 222)";
    super::ledger::finish(core, deal, Step::Closed, why).await;
    false
}

/// Фактическое количество монеты сделки на площадке. Ошибки — текст (шлюз, счёт не прочитан).
async fn fact(core: &Core, deal: &Deal) -> Result<Decimal, String> {
    let (v, a) = (&deal.at, &deal.direction.asset);
    let gw = super::dex_account::venue_of(core, deal, v).await?;
    if gw.kind() != VenueKind::Cex {
        return super::manual_halted::free_of(core, gw.as_ref(), v, a).await;
    }
    let on = super::venue_ticker::ticker_on(core, a, v);
    let accs = gw
        .account_balances()
        .await
        .map_err(|e| format!("{}: балансы счетов: {}", v.0, e.message))?;
    let mut sum = Decimal::ZERO;
    for acc in accs
        .iter()
        .filter(|x| gw.account_role(&x.account) != AccountRole::Other)
    {
        if let Some(e) = &acc.error {
            return Err(format!("{}: счёт «{}» не прочитан: {e}", v.0, acc.account));
        }
        sum += acc
            .balances
            .iter()
            .filter(|b| b.asset == on)
            .map(|b| b.free + b.locked)
            .sum::<Decimal>();
    }
    Ok(sum)
}

/// Запись в журнал core.remain_fact. Ошибок нет.
async fn log(core: &Core, deal: &Deal, text: &str) {
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
