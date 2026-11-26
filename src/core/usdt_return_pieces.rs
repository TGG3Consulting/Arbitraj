//! Файл: core/usdt_return_pieces.rs
//! Что делает: решение 394 (ревью 3 t179, Б1) — общий возврат USDT группы докупки несколькими выводами, когда сумма
//! выше максимума вывода площадки (coin_group_flush.rs pieces). Было (840f2557): k перегонов (Job) стартовали сразу, а
//! учёт хранит одну часть на площадку (usdt_return_parts::put) — куски затирали друг друга: «вернулись» одного куска
//! стирал «ждёт повтора» или «не вернулись» другого (USDT остаются на площадке, бот молчит), затраты — одного куска,
//! перезапуск — «не вернулись» только на один кусок; у кусков был один ID «<сделка>u». Теперь:
//! - куски уходят ПО ОЧЕРЕДИ в одной задаче перегона и ОДНОЙ части учёта; следующий — только после итога предыдущего
//!   (relay_try возвращает итог после зачисления на площадке покупки — transfers::send_as);
//! - у каждого куска свой ID вывода «<сделка>up<номер>» (один вывод — «<сделка>u», как раньше; повторы —
//!   «<сделка>u<номер>», ручной «Перегнать» — «<сделка>m<мс>…»); адаптеры бирж ставят в начало ID метку попытки (мс);
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
//! Ревью 4 (Н8): «Перегнать» остатка после сбоя куска — если остаток выше максимума вывода, теми же кусками по очереди
//! (start_manual, из manual_usdt.rs); сумма — дошедшее + остаток, номера кусков продолжаются, у ID — метка «m<мс>».
//! Кто вызывает: usdt_return_job.rs (Job с кусками), manual_usdt.rs (start_manual); ok / failed — ещё тесты.
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

/// Что делает: ID вывода куска (номер с 1) общего возврата сделки. Принимает: ID сделки, метку попытки («» — шаг
/// закрытия, «m<мс>» — «Перегнать»), номер. Ошибок нет.
pub fn piece_id(deal: &str, tag: &str, n: usize) -> String {
    format!("{deal}{tag}up{n}")
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
            result_at_failure: None,
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

/// Что делает: куски общего возврата по очереди (порядок — в шапке); страховка задачи (Guard) держит остаток. Сумма
/// общего возврата — уже дошедшее (DealGroup.pieces; у «Перегнать» после сбоя куска) + куски этой задачи; номера
/// кусков продолжают дошедшие. Принимает: ядро, перегон с кусками, страховку. Ошибок нет: сбой куска — «не вернулись»
/// на остаток со страховкой.
pub(super) async fn run(core: &Core, job: &Job, guard: &mut Guard) {
    let (base, done_n) = lk(&core.inner.deals)
        .get(&job.deal)
        .and_then(|d| d.group.as_ref())
        .and_then(|g| g.pieces.as_ref())
        .map_or((Decimal::ZERO, 0), |p| (p.amount, p.n as usize));
    let rem: Decimal = job.pieces.iter().copied().sum();
    let total = base + rem;
    let k = done_n + job.pieces.len();
    let way = (&job.from, &job.to);
    let mut sent = Decimal::ZERO;
    for (i, piece) in job.pieces.iter().copied().enumerate() {
        let no = done_n + i + 1;
        let id = piece_id(&job.deal, &job.tag, no);
        let res =
            super::usdt_return::relay_try(core, &id, way.0, way.1, job.acc.as_ref(), piece).await;
        match res {
            Ok(r) => {
                sent += piece;
                guard.amount = rem - sent;
                let note = format!(
                    "USDT вернулись: кусок {no} из {k} — {piece} {} → {}, зачислено {}{} (решение 394)",
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
                    "общий возврат группы: кусок {no} из {k} ({piece}) не дошёл: {e}{unknown}; остаток не отправлен"
                );
                let rest = commit(core, &job.deal, |d| failed(d, way.0, total, &why))
                    .await
                    .unwrap_or(rem - sent);
                st::alarm(core, &job.deal, rest, way.0, &why).await;
                return;
            }
        }
    }
    guard.done = true;
}

/// Что делает: ревью 4 t179, Н8 — «Перегнать» остатка общего возврата группы (часть уже «идёт» на `amount`, её
/// поставил manual_usdt.rs): остаток выше максимума вывода площадки — теми же кусками по очереди (run), а не одним
/// выводом с заведомым отказом площадки. Только у сделки с группой; перегон запрещён правилом биржа ↔ биржа, нет
/// кошелька или кусок один — false (вызывающий делает обычную попытку, она и сообщит причину). Принимает: ядро, ID
/// сделки, сумму, откуда, куда, метку попытки. Возвращает: true — куски запущены. Ошибок нет.
pub(super) fn start_manual(
    core: &Core,
    deal: &str,
    amount: Decimal,
    (from, to): (&VenueId, &VenueId),
    tag: &str,
) -> bool {
    let Some(d) = lk(&core.inner.deals)
        .get(deal)
        .filter(|x| x.group.is_some())
        .cloned()
    else {
        return false;
    };
    if super::usdt_return_deal::return_block(core, &d, from, to).is_some() {
        return false;
    }
    let (_, split) = super::coin_group_flush::return_parts(core, from, to, amount);
    let Ok(acc) = super::dex_account::of(core, &d) else {
        return false;
    };
    if split.len() < 2 {
        return false;
    }
    Job {
        deal: deal.to_string(),
        amount,
        from: from.clone(),
        to: to.clone(),
        acc,
        pieces: split.into_iter().map(|(_, a)| a).collect(),
        tag: tag.to_string(),
    }
    .start(core);
    true
}
