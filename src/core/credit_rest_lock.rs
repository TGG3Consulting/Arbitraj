//! Файл: core/credit_rest_lock.rs
//! Что делает: решение 413 (шестое и седьмое ревью t198) — решение остатка «не присвоено» под ОДНИМ замком счёта
//! (remain_read::with_fact: чтение баланса, учёт других и запись решения в учёт памяти — без окна для продажи другой
//! сделки). Совместно (Б-1): сделка и все другие сделки с остатком на счёте, что не в работе, — пул делится между ними
//! (credit_rest::share_out), решение каждой записывается в учёт памяти сразу и затем в базу (по свежей записи памяти —
//! действие человека между ними не перетирается). Уже решён другой сделкой — берётся из памяти. guard (круг сверки,
//! ожидание после продажи): запись сделки в памяти изменилась (взял человек) — Taken, без решения (С-2, под замком до
//! записи). Итог несёт прочитанный факт и учёт других — вызывающий без второго чтения идёт обычным путём (С-1).
//! Кто вызывает: core/remain_fact.rs (reconcile_as), core/credit_rest_park.rs, core/manual.rs, core/manual_halted.rs.
//! От чего зависит: credit_rest.rs (busy, share_out, apply_one), remain_read.rs (with_fact), credit_lock.rs (sync),
//! credit_fact.rs, deposit_hold.rs, credit_rest_value.rs (оценка после решения — ревью слияния 413×414), ledger.rs,
//! deps.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::credit_rest::{Resolved, apply_one, busy, share_out};
use super::deal::Deal;
use super::engine::{Core, lk};
use super::remain_others::Others;

/// Итог решения остатка.
pub enum Res {
    /// Решён (количество обновлено, остатка нет).
    Done,
    /// Счёт не тихий — причина словами.
    Busy(String),
    /// Баланс не прочитан — причина.
    Unread(String),
    /// Запись сделки в памяти изменилась (взял человек) — не решаем.
    Taken,
}

/// Итог с прочитанным фактом и учётом других.
pub struct Locked {
    pub res: Res,
    pub fact: Result<Decimal, String>,
    pub others: Others,
    /// Другие сделки, решённые совместно (их записи уже в учёте).
    pub joint: Vec<String>,
}

struct Allocation {
    idle: Vec<(String, Decimal, Decimal)>,
    prior: Decimal,
    shares: Vec<Decimal>,
}

// Единое распределение для записи факта и его предварительного просмотра.
fn allocation(deal: &Deal, f: Decimal, o: &Others) -> Allocation {
    let idle: Vec<(String, Decimal, Decimal)> = o
        .rests
        .iter()
        .filter(|x| x.3)
        .map(|x| (x.0.clone(), x.1, x.2))
        .collect();
    let theirs: Decimal = idle.iter().map(|x| x.1).sum();
    let rest = super::credit_fact::rest(deal).unwrap_or_default();
    // Восьмое ревью (С-1): своя с неизвестным исходом продажи — база 0 (её количество не доказано): получает только
    // после подтверждённых соучастников, верхняя граница та же.
    let own = if deal.sale_unsure {
        (Decimal::ZERO, deal.qty + rest)
    } else {
        (deal.qty, rest)
    };
    let mut parts = vec![own];
    parts.extend(idle.iter().map(|x| (x.1, x.2)));
    let scale = parts
        .iter()
        .flat_map(|p| [p.0.scale(), p.1.scale()])
        .chain([f.scale()])
        .max()
        .unwrap_or(0);
    let prior = o.qty - theirs;
    let shares = share_out(f - prior, &parts, scale);
    Allocation {
        idle,
        prior,
        shares,
    }
}

/// Прогноз доли без изменения учёта. Вызывающий держит замок факта.
pub(super) fn preview(
    core: &Core,
    deal: &Deal,
    f: Decimal,
    others: &Others,
) -> Result<Decimal, String> {
    if let Some(why) = busy(core, deal, others) {
        return Err(format!(
            "Счёт занят: {why}. Запросите цену после сверки остатка."
        ));
    }
    Ok(allocation(deal, f, others).shares[0])
}

/// Что делает: решение под замком (шапка). Принимает: ядро, сделку, guard. Возвращает: Locked. Ошибок нет.
pub async fn locked(core: &Core, deal: &mut Deal, guard: bool) -> Locked {
    let probe = deal.clone();
    let hold = super::deposit_hold::expected_pct(core, &deal.at, &deal.direction.asset);
    let mut texts: Vec<Resolved> = Vec::new();
    let mut joint: Vec<String> = Vec::new();
    // Ревью слияния 413×414 (Н-1): количества до решения — своей (только если решала эта сделка) и соучастников.
    let mut mine: Option<Decimal> = None;
    let mut was: Vec<(String, Decimal)> = Vec::new();
    let (res, fact, others) = Box::pin(super::remain_read::with_fact(core, &probe, |r, o| {
        let mem = lk(&core.inner.deals).get(&deal.id).cloned();
        // Уже решён совместно другой сделкой — из памяти (восьмое ревью С-4: при guard шаг тот же, иначе взял человек;
        // переносятся и хронология, и отметка «исход неизвестен»).
        if let Some(x) = &mem
            && x.credit.is_some()
            && super::credit_fact::rest(x).is_none()
            && super::credit_fact::rest(deal).is_some()
        {
            if guard && x.step != deal.step {
                return (Res::Taken, r, o);
            }
            (deal.qty, deal.credit) = (x.qty, x.credit.clone());
            (deal.timeline, deal.sale_unsure) = (x.timeline.clone(), x.sale_unsure);
            // Ревью dd29ef5a (С-1): и оценка с начислениями дня (пересчёт соучастника, credit_rest_value.rs) — иначе
            // сохранение вызывающего вернуло бы прежнюю оценку.
            (deal.provisional_usd, deal.day_book) = (x.provisional_usd, x.day_book.clone());
            return (Res::Done, r, o);
        }
        if guard && mem.map(|x| (x.step, x.updated_at)) != Some((deal.step, deal.updated_at)) {
            return (Res::Taken, r, o);
        }
        if let Some(why) = busy(core, deal, &o) {
            return (Res::Busy(why), r, o);
        }
        let f = match &r {
            Ok(f) => *f,
            Err(e) => return (Res::Unread(e.clone()), r, o),
        };
        let Allocation {
            idle,
            prior,
            shares,
        } = allocation(deal, f, &o);
        let peers: Vec<String> = idle
            .iter()
            .zip(shares.iter().skip(1))
            .map(|(x, q)| format!("{} → {q}", x.0))
            .collect();
        mine = Some(deal.qty);
        texts.push(apply_one(
            deal,
            shares[0],
            f,
            prior,
            hold,
            &peers.join(", "),
        ));
        super::credit_lock::sync(core, deal);
        let mut m = lk(&core.inner.deals);
        for (x, q) in idle.iter().zip(shares.iter().skip(1)) {
            if let Some(d) = m.get_mut(&x.0) {
                was.push((x.0.clone(), d.qty));
                let me = format!("{} → {}", deal.id, shares[0]);
                texts.push(apply_one(d, *q, f, prior, hold, &me));
                joint.push(x.0.clone());
            }
        }
        (Res::Done, r, o)
    }))
    .await;
    // Ревью слияния 413×414 (Н-1): количество изменилось у остановленной с оценкой — оценка в итоге дня заново
    // (credit_rest_value.rs, тот же daily::provisional, что у ledger::finish); иначе итог дня и «своё» в слепке просадки
    // (решение 414) остались бы по прежнему количеству.
    if let Some(before) = mine {
        Box::pin(super::credit_rest_value::revalue_own(core, deal, before)).await;
    }
    for (id, before) in &was {
        let fresh = Box::pin(super::credit_rest_value::revalue(core, id, *before))
            .await
            .or_else(|| lk(&core.inner.deals).get(id).cloned());
        if let Some(d) = fresh {
            Box::pin(super::ledger::save(core, &d)).await;
        }
    }
    let (deps, v) = (&core.inner.deps, deal.at.0.clone());
    for t in texts {
        if let Some(x) = t.over {
            Box::pin(deps.log(Level::Info, "core.credit_fact", Some(&v), &x, None)).await;
        }
        if let Some(x) = t.short {
            Box::pin(deps.log(Level::Warn, "core.credit_fact", Some(&v), &x, None)).await;
            Box::pin(deps.notify("deal.credit_hold", Some(&v), &x)).await;
        }
    }
    Locked {
        res,
        fact,
        others,
        joint,
    }
}

/// Что делает: кнопка продажи / «Беру на себя» у сделки с остатком (седьмое ревью): решение под замком; не тихо —
/// «счёт занят», не прочитан — причина; решено без монет — фантом закрыт или (на счёте только чужое) алерт человеку.
/// Принимает: ядро, сделку. Возвращает: Ok(None) — остатка нет или решён с монетами: продолжать; Ok(Some(текст)) —
/// сделка закрыта / передана человеку; Err(причина) — не решить сейчас. Ошибок сверх этого нет.
pub async fn for_button(core: &Core, deal: &mut Deal) -> Result<Option<String>, String> {
    if super::credit_fact::rest(deal).is_none() {
        return Ok(None);
    }
    let l = Box::pin(locked(core, deal, false)).await;
    match l.res {
        Res::Busy(w) => Err(format!(
            "счёт занят: {w} — остаток «не присвоено» сейчас не решить"
        )),
        Res::Unread(e) => Err(format!("баланс биржи не прочитан: {e}")),
        Res::Taken => Err("сделку уже взял человек".into()),
        Res::Done if deal.qty > Decimal::ZERO => Ok(None),
        Res::Done => match l.fact {
            Ok(f) => {
                let why = "остатка зачисления на счёте нет — фантом учёта (решение 222, 413)";
                Box::pin(super::remain_rest::empty(core, deal, f, &l.others, why)).await;
                let what = if f > Decimal::ZERO {
                    "на счёте только монеты других сделок — алерт человеку (решение 405)"
                } else {
                    why
                };
                Ok(Some(format!("Сделка {}: {what}", deal.id)))
            }
            Err(e) => Err(format!("баланс биржи не прочитан: {e}")),
        },
    }
}
