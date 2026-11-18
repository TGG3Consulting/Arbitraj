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
//! Больше учёта не берётся никогда. Решение 405 (t191, Тигран 18.11: «каждая сделка знает своё количество и работает
//! только с ним»): из факта вычитаются монеты ДРУГИХ сделок той же монеты, доказуемо лежащие на этом счёте
//! (remain_others.rs: не «Беру на себя»; сделка на шаге «перевод» — «неясно») — остаток = min(учёт, факт − других).
//! Пример: учёт 1000, на кошельке 900, другая сделка того же кошелька продаёт там 500 — дальше 400 (было 900, из них
//! 500 чужие). Факт есть, а за вычетом других ≤ 0 — сделка не закрывается и ничего не продаёт: алерт человеку
//! (all_foreign). Сделка в пути через счёт («неясно») не вычитается и вычет доказуемых не отключает (ревью 4, Н1).
//! Факта нет (баланс не прочитан, счёт с ошибкой) — учёт как раньше, строка в хронологии и журнал core.remain_fact.
//! На площадке ничего нет — сделка закрывается (остатка нет; возврат USDT — общий шаг закрытия ledger::finish).
//! Нативная BNB на DEX (решение 62) не сверяется: её баланс — общий с резервом на газ (native.rs, отдельные правила).
//! Решение 413 (t198): чтение факта и учёта других — под исключительным замком счёта (credit_lock.rs: продажи в полёте
//! нет); сделки, чьё зачисление ждёт доли (credit_fact.rs), получают долю распределением факта (credit_split.rs,
//! credit_apply.rs) — своя ждущая подтверждается долей, подтверждённая вычитает доли ждущих других. Факт не прочитан, а
//! своя ждёт доли — повторное ревью (Б-3): на бирже не продаём — режим застрявшей монеты один раз, потом алерт
//! (credit_alert::unread); чтение с повторами по правилам шлюза — remain_read.rs.
//! Кто вызывает: core/stuck.rs (handle), core/sell_unsent.rs, core/sell_fact.rs (перед продажей на бирже),
//! core/credit_base.rs.
//! От чего зависит: manual_halted.rs (free_of, set_qty), remain_others.rs, remain_alert.rs (всё чужое), dex_account.rs,
//! venue_ticker.rs, native.rs, ledger.rs, deal.rs, credit_lock.rs, credit_split.rs, credit_apply.rs, credit_fact.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Сколько вести дальше: min(учёт, факт), не меньше нуля. Принимает: учёт сделки, факт площадки. Ошибок нет.
pub fn settle_qty(account: Decimal, fact: Decimal) -> Decimal {
    account.min(fact).max(Decimal::ZERO)
}

/// Сверка остатка сделки с фактом площадки (решение 222). Принимает: ядро, сделку (количество меняется). Возвращает:
/// true — вести дальше; false — на площадке ничего нет, сделка закрыта, или (решение 405) по учёту всё на счёте —
/// монеты других сделок: алерт человеку. Ошибок нет: факта нет — учёт как раньше.
pub async fn reconcile(core: &Core, deal: &mut Deal) -> bool {
    let mode = if super::risk_count::is_manual() {
        super::credit_alert::Unread::Button
    } else {
        super::credit_alert::Unread::Stuck
    };
    Box::pin(reconcile_as(core, deal, mode)).await
}

/// Что делает: как reconcile, с заданным поведением при непрочитанном балансе биржи (третье ревью t198: кнопка — итог
/// кнопки с причиной, С-Г; первый вывод — ждать и перечитывать, С-Д; прочее — режим застрявшей монеты один раз, потом
/// алерт, Б-3). Принимает: ядро, сделку, поведение. Возвращает: как reconcile. Ошибок нет.
pub async fn reconcile_as(core: &Core, deal: &mut Deal, mode: super::credit_alert::Unread) -> bool {
    if deal.qty <= Decimal::ZERO || super::native::is_native(core, &deal.direction.asset) {
        return true;
    }
    let cex = super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex);
    // Решение 413: факт и учёт других — одним снимком под замком счёта (продажи в полёте нет); на бирже — повторы по
    // правилу шлюза «повторить» (remain_read.rs).
    let (mut read, mut others) = Box::pin(super::remain_read::read(core, deal)).await;
    // Третье ревью (С-Д), четвёртое (С-4): первый вывод — ждём и перечитываем не дольше transfer.max_time_s
    // (fact_wait.rs); сделку взял человек — дальше не ведём; предел вышел — режим застрявшей монеты с причиной.
    if cex
        && mode == super::credit_alert::Unread::Wait
        && let Err(e) = &read
    {
        let waited = Box::pin(super::fact_wait::first(core, deal, e.clone(), others)).await;
        match waited {
            super::fact_wait::Waited::Read(r, o) => (read, others) = (r, o),
            super::fact_wait::Waited::Gone => return false,
        }
    }
    let fact = match read {
        Ok(f) => f,
        // Повторное ревью (Б-3): на бирже факт не прочитан — не продаём и не выводим (не вслепую).
        Err(e) if cex => {
            return Box::pin(super::credit_alert::unread(core, deal, &e, mode)).await;
        }
        Err(e) => {
            let text =
                format!("Сверка остатка с балансом площадки не вышла: {e} — по учёту сделки");
            log(core, deal, &text).await;
            deal.note(text);
            return true;
        }
    };
    deal.fact_unread = false;
    // Четвёртое ревью (С-7): факт этой сделки прочитан — её учёт сверен с балансом, отметка «исход неизвестен» снята.
    super::credit_lock::settle(&deal.id);
    let ok = Box::pin(apply_with(core, deal, fact, others)).await;
    // Повторное ревью (мелкое): факт свежий при этом шаге, количестве и площадке — второй вход подряд не читает.
    if ok {
        deal.fact_fresh = Some(Box::new((deal.step, deal.qty, deal.at.clone())));
    }
    ok
}

/// Что делает: решение по прочитанному факту счёта (вынесено из reconcile — проверяется тестами на записях учёта без
/// площадок, ревью 3 М5): min(учёт, факт − доказуемых других) («неясные» не вычитаются) / всё чужое — алерт / пусто —
/// закрыть.
/// Принимает: ядро, сделку (количество меняется), факт. Возвращает: как reconcile. Ошибок нет.
pub async fn apply_fact(core: &Core, deal: &mut Deal, fact: Decimal) -> bool {
    // Решение 405 (t191): монеты других сделок той же монеты на этом счёте — не наши (remain_others.rs).
    let others = super::remain_others::on_account(core, deal, &deal.at);
    Box::pin(apply_with(core, deal, fact, others)).await
}

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
    // Четвёртое ревью (С-3, С-5): у сделки остаток «не присвоено, перечитать» — снова ждёт доли (количество + остаток).
    super::credit_fact::reopen(deal);
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
    let why = "остатка на площадке нет — фантом учёта (решение 222)";
    super::ledger::finish(core, deal, Step::Closed, why).await;
    false
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
