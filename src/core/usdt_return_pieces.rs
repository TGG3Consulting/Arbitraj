//! Файл: core/usdt_return_pieces.rs
//! Что делает: решение 394 (ревью 3 t179, Б1) — общий возврат USDT группы докупки несколькими выводами, когда сумма
//! выше максимума вывода площадки (coin_group_flush.rs pieces). Было (840f2557): k перегонов (Job) стартовали сразу, а
//! учёт хранит одну часть на площадку (usdt_return_parts::put) — куски затирали друг друга: «вернулись» одного куска
//! стирал «ждёт повтора» или «не вернулись» другого (USDT остаются на площадке, бот молчит), затраты — одного куска,
//! перезапуск — «не вернулись» только на один кусок; у кусков был один ID «<сделка>u». Теперь:
//! - куски уходят ПО ОЧЕРЕДИ в одной задаче перегона и ОДНОЙ части учёта; следующий — только после итога предыдущего
//!   (relay_try возвращает итог после зачисления на площадке покупки — transfers::send_as);
//! - у каждого куска свой ID вывода «<сделка>up<номер>» (один вывод — «<сделка>u», как раньше; повторы —
//!   «<сделка>u<номер>», ручной «Перегнать» — свой тег); адаптеры бирж ставят в начало ID метку попытки (мс);
//! - кусок дошёл: итог дошедших — DealGroup.pieces (их затраты сразу в итоге сделки, deal_return_cost::known), часть
//!   учёта — «идёт» на остаток; запись сохраняется — перезапуск посреди кусков: «не вернулись» на точный остаток
//!   (usdt_return_state::on_restart), дошедшее учтено;
//! - дошли все — часть «вернулись» на всю сумму: ушло, зачислено и газ — суммы по кускам (комиссия вывода каждого
//!   куска — в итоге сделки, записка 269, п. 14);
//! - кусок не дошёл (отказ площадки, исход неизвестен, нет данных) — «не вернулись» на точный остаток (этот кусок и не
//!   начатые) со страховкой: журнал core.usdt_not_returned, Telegram usdt.not_returned, запись с кнопками на «Ручное
//!   управление». Повторов у кусков нет (повтор ищет часть по площадке и шлёт её сумму одним выводом — выше
//!   максимума): решает человек.
//!
//! Кто вызывает: usdt_return_job.rs (Job с кусками); ok / failed — ещё тесты.
//! От чего зависит: usdt_return.rs (relay_try), usdt_return_state.rs, usdt_return_parts.rs, deal_return_cost.rs,
//! deal_closed.rs, group_notice.rs, ledger.rs (save), deal_group.rs.
//! Настройки: нет (максимум вывода — справочник сетей площадки, coin_group_flush.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::deal::Deal;
use super::deal_return_cost::Relayed;
use super::engine::{Core, lk};
use super::usdt_return_job::{Guard, Job};
use super::usdt_return_state::{self as st, UsdtReturn};

/// Что делает: ID вывода куска (номер с 1) общего возврата сделки. Принимает: ID сделки, номер. Ошибок нет.
pub fn piece_id(deal: &str, n: usize) -> String {
    format!("{deal}up{n}")
}

/// Что делает: кусок дошёл — его итог в DealGroup.pieces; дошли все (остаток 0) — часть «вернулись» на всю сумму с
/// суммами по кускам, DealGroup.pieces снимается; иначе часть «идёт» на остаток. Принимает: сделку (с группой),
/// (откуда, куда), сумму общего возврата, сумму куска, итог перегона куска. Возвращает: остаток. Ошибок нет.
pub fn ok(
    d: &mut Deal,
    (from, to): (&VenueId, &VenueId),
    total: Decimal,
    piece: Decimal,
    r: &Relayed,
) -> Decimal {
    let mut acc = d
        .group
        .as_ref()
        .and_then(|g| g.pieces.clone())
        .unwrap_or_default();
    acc.n += 1;
    acc.amount += piece;
    acc.sent += r.sent;
    acc.credited += r.credited;
    acc.fee_usd = match (acc.fee_usd, r.fee_usd) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or_default() + b.unwrap_or_default()),
    };
    let rest = (total - acc.amount).max(Decimal::ZERO);
    let (state, keep) = if rest.is_zero() {
        let done = UsdtReturn::Done {
            amount: acc.amount,
            credited: acc.credited,
            from: from.clone(),
            to: to.clone(),
            sent: Some(acc.sent),
            fee_usd: acc.fee_usd,
        };
        (done, None)
    } else {
        let wait = UsdtReturn::Pending {
            amount: rest,
            from: from.clone(),
            to: to.clone(),
        };
        (wait, Some(acc))
    };
    if let Some(g) = d.group.as_mut() {
        g.pieces = keep;
    }
    super::usdt_return_parts::put(d, state);
    rest
}

/// Что делает: кусок не дошёл — «не вернулись» на остаток (этот кусок и не начатые; дошедшие учтены в DealGroup.pieces),
/// строка в хронологию. Принимает: сделку, откуда, сумму общего возврата, причину. Возвращает: остаток. Ошибок нет.
pub fn failed(d: &mut Deal, from: &VenueId, total: Decimal, why: &str) -> Decimal {
    let done = d
        .group
        .as_ref()
        .and_then(|g| g.pieces.as_ref())
        .map_or(Decimal::ZERO, |p| p.amount);
    let rest = total - done;
    d.note(st::not_returned_text(&d.id, rest, from, why));
    super::usdt_return_parts::put(
        d,
        UsdtReturn::NotReturned {
            amount: rest,
            at: from.clone(),
            why: why.to_string(),
        },
    );
    rest
}

/// Что делает: правка сделки в учёте под замком (`f`), затраты возврата в итог, сохранение, отложенные уведомления
/// (deal.closed, «Итог группы») — как usdt_return_state::set. Принимает: ядро, ID сделки, правку. Возвращает: что
/// вернула правка; None — сделки нет в учёте. Ошибок нет.
async fn commit<T>(core: &Core, deal: &str, f: impl FnOnce(&mut Deal) -> T) -> Option<T> {
    let (d, delta, notice, total, out) = {
        let mut deals = lk(&core.inner.deals);
        let x = deals.get_mut(deal)?;
        let out = f(x);
        let delta = super::deal_return_cost::apply(x);
        let notice = super::deal_closed::take_ready(x);
        let total = super::group_notice::take(x);
        (x.clone(), delta, notice, total, out)
    };
    super::ledger::save(core, &d).await;
    super::deal_return_cost::after(core, &d, delta).await;
    if let Some(why) = notice {
        super::deal_closed::send(core, &d, &why).await;
    }
    if let Some(t) = total {
        super::group_notice::send(core, &d, &t).await;
    }
    Some(out)
}

/// Что делает: куски общего возврата по очереди (порядок — в шапке); страховка задачи (Guard) держит остаток.
/// Принимает: ядро, перегон с кусками, страховку. Ошибок нет: сбой куска — «не вернулись» на остаток со страховкой.
pub(super) async fn run(core: &Core, job: &Job, guard: &mut Guard) {
    let k = job.pieces.len();
    let total: Decimal = job.pieces.iter().copied().sum();
    let way = (&job.from, &job.to);
    let mut sent = Decimal::ZERO;
    for (i, piece) in job.pieces.iter().copied().enumerate() {
        let id = piece_id(&job.deal, i + 1);
        let res =
            super::usdt_return::relay_try(core, &id, way.0, way.1, job.acc.as_ref(), piece).await;
        match res {
            Ok(r) => {
                sent += piece;
                guard.amount = total - sent;
                let note = format!(
                    "USDT вернулись: кусок {} из {k} — {piece} {} → {}, зачислено {}{} (решение 394)",
                    i + 1,
                    job.from.0,
                    job.to.0,
                    r.credited,
                    r.tail()
                );
                commit(core, &job.deal, |d| {
                    d.note(note);
                    ok(d, way, total, piece, &r)
                })
                .await;
            }
            Err(e) => {
                guard.done = true;
                let unknown = if e.outcome_unknown() {
                    "; исход этого куска неизвестен — перевод мог уйти, проверьте обе площадки"
                } else {
                    ""
                };
                let why = format!(
                    "общий возврат группы: кусок {} из {k} ({piece}) не дошёл: {e}{unknown}; остаток не отправлен",
                    i + 1
                );
                let rest = commit(core, &job.deal, |d| failed(d, way.0, total, &why))
                    .await
                    .unwrap_or(total - sent);
                st::alarm(core, &job.deal, rest, way.0, &why).await;
                return;
            }
        }
    }
    guard.done = true;
}
