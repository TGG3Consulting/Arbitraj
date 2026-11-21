//! Файл: core/coin_group_flush.rs
//! Что делает: решение 394 (t179) — ОДИН общий возврат USDT группы докупки (вынесено из coin_group.rs — размер файла;
//! ревью 2 t179 — Н1, ревью 3 — Б1, Н2, Н4). Порядок:
//! - носитель — первая сделка группы, если её часть в пуле, иначе первая отложенная; в копиях записей (из учёта под
//!   замком) у всех частей снимается wait, у остальных — «общий возврат в сделке …» (shared_in), у носителя — итог
//!   группы (GroupSum); в учёт памяти копии и носитель (с частью «идёт») попадают одним захватом замка перед
//!   сохранениями (решение 489: держатель сети не пропадает, сумма не задваивается), в базу — сохранением;
//! - перегон готовится у носителя ОДНОЙ частью учёта на всю сумму (состояние «идёт»); сумма выше максимума вывода
//!   площадки (справочник сетей ядра) — куски не больше максимума (pieces), уходят ПО ОЧЕРЕДИ в той же задаче
//!   перегона, каждый со своим ID вывода (usdt_return_pieces.rs); ниже минимума — одним выводом, как у одиночной;
//! - носитель уже известил о своём закрытии — «Итог группы» уйдёт отдельным сообщением по концу возврата
//!   (group_notice.rs, Н4);
//! - сохраняется СНАЧАЛА носитель (Pending на всю сумму и список сделок группы в GroupSum.deals), потом остальные
//!   (Н2); только потом перегон стартует.
//!
//! Падение процесса: до сохранения носителя — у всех сделок wait → при запуске «не вернулись» по своим суммам; после
//! носителя, до остальных — у остальных wait, но их ID есть в GroupSum.deals сохранённого носителя → при запуске они
//! «в общем возврате в сделке …» (ledger_load.rs), у носителя — «не вернулись» на всю сумму (ничего не числится
//! дважды и ничего не теряется); после всех — у носителя «не вернулись» на всю сумму (или на остаток кусков), у
//! остальных возврата нет («Перегнать» по сделке с общим возвратом — отказ, manual_usdt.rs retry_block).
//! Кто вызывает: coin_group.rs (after_close, spawn_if_last).
//! От чего зависит: coin_group.rs (Part), deal_group.rs, usdt_return_deal.rs (start_parts), group_notice.rs,
//! deal_closed.rs, ledger.rs (save), usdt_return.rs (USDT площадки), transfers.rs (сеть вывода), dirctx_venue.rs
//! (запись справочника сетей), engine.rs.
//! Настройки: через usdt_return.rs и transfers.rs (venue.quote_asset, venue.bsc_network).

use rust_decimal::Decimal;

use crate::core::types::VenueId;

use super::coin_group::Part;
use super::deal_group::GroupSum;
use super::engine::{Core, lk};

/// Что делает: максимум вывода USDT площадки возврата `v` — в той же сети, которой идёт сам вывод (ревью 4 t179, Н11):
/// USDT площадки — как у перегона (usdt_return::usdt: тикер venue.quote_asset, адрес USDT ядра), сеть — как у вывода
/// (transfers::network: своя запись справочника сетей площадки, иначе venue.bsc_network), запись справочника — по
/// тикеру и этой сети (dirctx_venue::quote_network). Было: сеть прямо из venue.bsc_network — у площадки, чья запись
/// справочника названа иначе, максимум брался из другой записи или не находился. Принимает: ядро, площадку.
/// Возвращает: максимум; нет данных (DEX, справочника нет, сеть не найдена) или 0 — None. Ошибок нет.
pub(super) fn max_withdraw(core: &Core, v: &VenueId) -> Option<Decimal> {
    let usdt = super::usdt_return::usdt(core, v).ok()?;
    let net = super::transfers::network(core, &usdt, v).ok()?;
    super::dirctx_venue::quote_network(core, v, &usdt.ticker, &net)
        .and_then(|i| i.withdraw_max)
        .filter(|m| !m.is_zero())
}

/// Что делает: сумма общего возврата кусками, каждый НЕ БОЛЬШЕ максимума вывода (ревью 4, Н10): кусок —
/// total / ⌈total / max⌉, округлённый вверх до 8 знаков, но не больше максимума (вниз до 8 знаков); куски такого
/// размера, пока остаток больше максимума, последний — остаток (он не больше максимума и больше нуля). Сумма кусков =
/// total. Сумма не выше максимума или максимума нет — один кусок. Принимает: площадку, сумму, максимум.
/// Возвращает: [(площадка, кусок)]. Ошибок нет.
pub fn pieces(from: &VenueId, total: Decimal, max: Option<Decimal>) -> Vec<(VenueId, Decimal)> {
    use rust_decimal::RoundingStrategy as R;
    let Some(max) = max.filter(|m| total > *m && *m > Decimal::ZERO) else {
        return vec![(from.clone(), total)];
    };
    let n = (total / max).ceil();
    let each = (total / n)
        .round_dp_with_strategy(8, R::AwayFromZero)
        .min(max.round_dp_with_strategy(8, R::ToZero));
    if each <= Decimal::ZERO {
        return vec![(from.clone(), total)];
    }
    let mut out = vec![];
    let mut left = total;
    while left > max {
        out.push((from.clone(), each));
        left -= each;
    }
    out.push((from.clone(), left));
    out
}

/// Что делает: общий возврат частей одним перегоном (порядок — в шапке). Принимает: ядро, группу, части, был ли flush
/// раньше (тогда это сделка, закрывшаяся после общего возврата, — «была частью группы», свой возврат). Ошибок нет.
pub(super) async fn flush(core: &Core, g: &str, parts: Vec<Part>, was: bool) {
    let Some(first) = parts.first().cloned() else {
        return;
    };
    let bearer = parts.iter().find(|p| p.deal == g).cloned().unwrap_or(first);
    let others: Vec<&Part> = parts.iter().filter(|p| p.deal != bearer.deal).collect();
    let total: Decimal = parts.iter().map(|p| p.amount).sum();
    // Решение 489: правки — только в копиях; в учёт они попадают сохранением (носитель первым — вместе с частью «идёт»
    // общего возврата). Снятое ожидание (wait) не видно в учёте раньше, чем появится эта часть: держатель сети площадки
    // возврата (net_hold.rs) не пропадает ни на миг.
    let (copies, lead) = {
        let deals = lk(&core.inner.deals);
        let mut copies = vec![];
        for p in &others {
            if let Some(d) = deals.get(&p.deal) {
                let mut d = d.clone();
                if let Some(x) = d.group.as_mut() {
                    x.wait = None;
                    x.shared_in = Some(bearer.deal.clone());
                }
                d.note(format!(
                    "Возврат USDT: 0 — общий возврат группы в сделке {} (решение 394)",
                    bearer.deal
                ));
                copies.push(d);
            }
        }
        let lead = deals.get(&bearer.deal).map(|d| {
            let mut d = d.clone();
            if let Some(x) = d.group.as_mut() {
                x.wait = None;
                x.shared_in = None;
                x.left = was;
                if !others.is_empty() {
                    x.sum = Some(GroupSum {
                        deals: others.iter().map(|p| p.deal.clone()).collect(),
                        others: others.iter().filter_map(|p| p.result).sum(),
                        usdt: others.iter().map(|p| p.amount).sum(),
                    });
                }
            }
            d
        });
        (copies, lead)
    };
    let Some(mut lead) = lead else {
        return;
    };
    let why = if was {
        format!("была частью группы {g}, общий возврат группы ушёл раньше — свой возврат")
    } else {
        format!(
            "общий возврат USDT группы {g}: {total} (сделки {}) — затраты возврата один раз, в этой сделке",
            parts
                .iter()
                .map(|p| p.deal.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    lead.note(format!("{why} (решение 394)"));
    let max = max_withdraw(core, &bearer.from);
    let mut jobs = Box::pin(super::usdt_return_deal::start_parts(
        core,
        &mut lead,
        vec![(bearer.from.clone(), total)],
    ))
    .await;
    // Ревью 3, Б1: выше максимума вывода — куски по очереди в ОДНОЙ задаче и одной части учёта (usdt_return_pieces.rs).
    let split = pieces(&bearer.from, total, max);
    if split.len() > 1
        && let Some(j) = jobs.first_mut()
    {
        lead.note(format!(
            "Возврат USDT {total} выше максимума вывода {} ({}) — {} выводами по очереди: {} (решение 394)",
            max.unwrap_or_default(),
            bearer.from.0,
            split.len(),
            split
                .iter()
                .map(|(_, a)| a.to_string())
                .collect::<Vec<_>>()
                .join(" + ")
        ));
        j.pieces = split.into_iter().map(|(_, a)| a).collect();
    }
    // Ревью 3, Н4: носитель уже известил о закрытии — «Итог группы» отдельным сообщением по концу возврата.
    super::group_notice::mark(&mut lead);
    // Перегон не начат (страховка «не вернулись») — отложенные уведомления носителя уходят сразу.
    let (notice, total_msg) = if jobs.is_empty() {
        (
            super::deal_closed::take_ready(&mut lead),
            super::group_notice::take(&mut lead),
        )
    } else {
        (None, None)
    };
    // Решение 489 (ревью 4): носитель и остальные — в учёт памяти одним захватом замка: читатели ожиданий и частей
    // (резерв возвратов usdt_hold.rs, просадка drawdown_pending.rs, держатель сети net_hold.rs) видят либо всё до
    // общего возврата, либо всё после — без задвоения суммы и без пропажи держателя. Порядок записи в базу — прежний.
    {
        let mut deals = lk(&core.inner.deals);
        deals.insert(lead.id.clone(), lead.clone());
        for d in &copies {
            deals.insert(d.id.clone(), d.clone());
        }
    }
    // Ревью 3, Н2: носитель — первым (см. шапку), потом остальные.
    Box::pin(super::ledger::save(core, &lead)).await;
    for d in &copies {
        Box::pin(super::ledger::save(core, d)).await;
    }
    if let Some(w) = notice {
        Box::pin(super::deal_closed::send(core, &lead, &w)).await;
    }
    if let Some(t) = total_msg {
        Box::pin(super::group_notice::send(core, &lead, &t)).await;
    }
    for j in jobs {
        j.start(core);
    }
}
