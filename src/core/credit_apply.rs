//! Файл: core/credit_apply.rs
//! Что делает: решение 413 (t198) — применение факта счёта биржи к сделке, чьё зачисление ждёт доли (credit_fact.rs):
//! распределение (credit_split.rs) по всем ждущим на этом счёте, подтверждение своей доли (количество сделки = доля),
//! замер удержания биржи (deposit_hold.rs), хронология / журнал core.credit_fact / уведомление deal.credit_hold с
//! цифрами: запись депозита, факт, удержание (шт и %), доля, другие сделки на счёте.
//! Своя доля 0 (на счёте нет монет ждущих) — алерт человеку («Ручное управление»), сделка не закрывается: запись
//! депозита есть, а монет нет — не «фантом», а необъяснённое (решает человек).
//! Баланс не прочитан (estimate) — оценка «запись × (1 − ожидаемое удержание)»: не больше доли сделки при удержании не
//! меньше ожидаемого (чужие монеты не продаются); остаток сверх оценки, если удержания не было, остаётся на счёте и
//! виден в хронологии.
//! Ревью t198 (С2): замер — только однозначный (measurable). Выход из «ждёт доли» на круге сверки — credit_halted.rs.
//! Уведомление, доля 0 и оценка — credit_alert.rs.
//! Кто вызывает: core/remain_fact.rs (apply_with, reconcile), core/credit_halted.rs (input).
//! От чего зависит: credit_split.rs, credit_fact.rs, credit_text.rs, credit_lock.rs (sync), deposit_hold.rs,
//! remain_others.rs, deal_words.rs, manual.rs (raise_alert_with), settings, deps.rs.
//! Настройки: transfer.credit_amount_tolerance_pct (допуск на округление, по площадке и монете; не задан — 0).

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_fact::Basis;
use super::credit_split::{Input, Pending, Split};
use super::deal::Deal;
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: входы распределения для счёта сделки: ждущие — другие ждущие и сама сделка, если ждёт; подтверждённые —
/// учёт других и свой. Принимает: ядро, сделку, факт, других. Возвращает: Input. Ошибок нет.
pub fn input(core: &Core, deal: &Deal, fact: Decimal, o: &Others) -> Input {
    let mut pending = o.pending.clone();
    // Третье ревью (С-Е): монеты сделок, уходящих с этого счёта (вывод ещё может не быть списан), — не в пуле ждущих.
    let mut confirmed = o.qty + o.leaving_qty;
    match super::credit_fact::here(deal).filter(|_| super::credit_fact::pending(deal)) {
        Some(c) => pending.push(Pending {
            id: deal.id.clone(),
            record: c.record,
            outsider: c.outsider,
            seen: c.outsider_at,
        }),
        None => confirmed += deal.qty,
    }
    let a = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&a.ticker, a.bsc_contract.as_ref());
    let at = At {
        venue: Some(&deal.at.0),
        coin: Some(&coin),
    };
    let tol = core
        .inner
        .deps
        .settings
        .decimal("transfer.credit_amount_tolerance_pct", at)
        .unwrap_or_default();
    Input {
        fact,
        confirmed,
        pending,
        // Сделка в пути сюда, замок не взят или сделка уходит отсюда (повторное ревью, С-1) — замер неоднозначен.
        unclear: !o.unclear.is_empty() || !o.leaving.is_empty(),
        hold_pct: super::deposit_hold::expected_pct(core, &deal.at, a),
        tol_pct: tol,
    }
}

/// Что делает: монеты других сделок на счёте с долями ждущих (для сверки остатка подтверждённой сделки): qty других +
/// доли других ждущих. Принимает: других, распределение. Возвращает: Others (pending перенесены в qty). Ошибок нет.
pub fn with_shares(o: &Others, sp: &Split) -> Others {
    let mut out = o.clone();
    for p in &o.pending {
        let q = sp.share_of(&p.id).unwrap_or_default();
        out.qty += q;
        out.deals.push((p.id.clone(), q));
    }
    out.pending.clear();
    out.deals.sort();
    out
}

/// Что делает: подтверждает долю своей ждущей сделки по распределению (шапка). Принимает: ядро, сделку, факт, других,
/// распределение. Возвращает: true — вести дальше с долей; false — доля 0, алерт человеку. Ошибок нет.
pub async fn own(core: &Core, deal: &mut Deal, fact: Decimal, o: &Others, sp: &Split) -> bool {
    let measured = measurable(core, deal, sp, sp.raw_of(&deal.id).unwrap_or_default());
    // Третье ревью (С-В): однозначный замер — удержание измерено на этой сделке: она берёт долю по факту без ограничения
    // ожидаемым (реальные 0 % — продаёт всё своё, а не 98 %).
    let share = if measured {
        sp.raw_of(&deal.id).unwrap_or_default()
    } else {
        sp.share_of(&deal.id).unwrap_or_default()
    };
    let record = super::credit_fact::here(deal).map_or(deal.qty, |c| c.record);
    let text = super::credit_text::describe(core, deal, fact, o, sp, (record, share));
    if share <= Decimal::ZERO {
        return Box::pin(super::credit_alert::zero(core, deal, o, text)).await;
    }
    super::credit_fact::confirm(deal, share, Basis::Balance);
    deal.note(text.clone());
    super::credit_lock::sync(core, deal);
    // Замер — сразу в памяти (расчёт следующих сделок видит его); база, журнал и Telegram — фоном: продажа их не ждёт
    // (Тигран 16.11: сделка не ждёт ни уведомления, ни записи в базу).
    let hold = measured.then(|| super::deposit_hold::Sample {
        records: sp.records,
        credited: sp.records - sp.held,
        pct: sp.held_pct(),
        at: chrono::Utc::now(),
        deals: sp.shares.iter().map(|(id, _)| id.clone()).collect(),
    });
    let (v, a) = (deal.at.clone(), deal.direction.asset.clone());
    if let Some(h) = &hold {
        super::deposit_hold::put(core, &v, &a, h.clone());
    }
    let tg = super::credit_alert::notice(deal, record, share);
    let c = core.clone();
    tokio::spawn(async move {
        let deps = &c.inner.deps;
        let lvl = if tg.is_some() {
            Level::Warn
        } else {
            Level::Info
        };
        deps.log(lvl, "core.credit_fact", Some(&v.0), &text, None)
            .await;
        if hold.is_some() {
            super::deposit_hold::save(&c, &v, &a).await;
        }
        if let Some(tg) = tg {
            deps.notify("deal.credit_hold", Some(&v.0), &tg).await;
        }
    });
    true
}

/// Что делает: ревью t198 (С2) — замер удержания пишется только однозначный: распределение однозначно (одна ждущая,
/// снимок посторонних, никого в пути, без лишнего — credit_split.rs measurable), доля — не пыль (не меньше минимального
/// ордера пары биржи по цене покупки сделки, dust.rs — минимумы пары из API биржи; цены покупки нет — не меряем), и это
/// не ручное действие (risk_count::is_manual: кнопка человека — сделка могла быть тронута руками). Принимает: ядро,
/// сделку, распределение, долю. Возвращает: да — писать замер. Ошибок нет.
fn measurable(core: &Core, deal: &Deal, sp: &Split, share: Decimal) -> bool {
    if !sp.measurable || super::risk_count::is_manual() {
        return false;
    }
    let Some(price) = deal.buy.as_ref().map(|b| b.avg_price) else {
        return false;
    };
    let on = super::venue_ticker::ticker_on(core, &deal.direction.asset, &deal.at);
    !super::dust::is_dust(core, &deal.at, &on, share, price)
}
