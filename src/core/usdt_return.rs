//! Файл: core/usdt_return.rs
//! Что делает: возврат USDT между биржей и кошельком (решение 34). Режим usdt_return.mode:
//! «в каждой сделке» (per_trade) — после закрытия сделки выручка переводится с B обратно на A
//! (стоимость перегона уже была в расчёте сделки, costs.rs); «отдельно» (separate) — при сверке
//! балансов: на площадке USDT меньше usdt_return.low_balance_usd — перегнать usdt_return.amount_usd
//! с площадки, где USDT хватает (решение 118: донор — только если у него после отдачи не меньше его порога; свободное
//! ведётся по ходу круга). «В каждой сделке» уходит выручка минус комиссия продажи в котируемой монете. Каждый перегон — уведомление usdt.relayed. Сухой режим (решение 74, dry.rs) — «отдельно»
//! не перегоняет; «в каждой сделке» — часть начатой сделки, она доводится как обычно.
//! Кнопка «Продать и перегнать» (решение 101, deal.return_usdt) — перегон выручки этой сделки при любом режиме.
//! Решение 138: возврат выручки закрытой сделки решает usdt_return_deal.rs (общий шаг ledger::finish), здесь — сам
//! перегон (relay) и режим «отдельно». Решение 236: перегон возвращает итог (Relayed: ушло, зачислено, газ в USDT) —
//! затраты возврата входят в итог сделки (deal_return_cost.rs).
//! Кто вызывает: usdt_return_job.rs (relay, relay_try), usdt_return_retry.rs (relay_try, решение 163), reconcile.rs (separate).
//! От чего зависит: transfers.rs, engine.rs (адрес USDT), dry.rs, deal_return_cost.rs (Relayed, газ в USDT), settings.
//! Решение 249: «отдельно» биржа → биржа — только если разрешено venue.cex_cex обеих и списком venue.cex_cex_deny
//! донора (cex_link::pair); иначе этот донор не берётся.
//! Настройки: usdt_return.mode, usdt_return.low_balance_usd, usdt_return.amount_usd (по площадке), venue.quote_asset;
//! venue.cex_cex, venue.cex_cex_deny (через cex_link.rs).

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Asset, DexAccountId, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal_return_cost::Relayed;
use super::engine::{Core, rd};
use super::op_error::OpError;

/// Держатель USDT в сверке: площадка и счёт DEX (None — биржа).
pub type Holder = (VenueId, Option<DexAccountId>);

/// USDT в BSC как актив (тикер — котируемая монета площадки, адрес — из справочника бирж).
pub(super) fn usdt(core: &Core, venue: &VenueId) -> Result<Asset, String> {
    let ticker = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let addr = rd(&core.inner.usdt).clone()?;
    Ok(Asset {
        ticker,
        bsc_contract: Some(addr),
    })
}

/// Перегон USDT с from на to. Принимает: …, счёт DEX перегона (решение 132; None — DEX не участвует).
/// Возвращает: итог перегона (решение 236: ушло, зачислено, газ в USDT). Ошибки: перегон не состоялся (OpError; уже в
/// журнале и usdt.relay_failed).
pub(super) async fn relay(
    core: &Core,
    id: &str,
    from: &VenueId,
    to: &VenueId,
    acc: Option<&DexAccountId>,
    amount: Decimal,
) -> Result<Relayed, OpError> {
    let res = relay_try(core, id, from, to, acc, amount).await;
    if let Err(e) = &res {
        let deps = &core.inner.deps;
        let text = format!(
            "Перегон USDT {amount}: {} → {} не состоялся: {e}. Деньги остались на {} — перегнать вручную",
            from.0, to.0, from.0
        );
        deps.log(Level::Error, "core.usdt_return", Some(&from.0), &text, None)
            .await;
        // Решение 108: раньше — только журнал; деньги молча оставались не на той площадке. Решение 164: текст Telegram.
        let tg =
            crate::notify::human::Msg::new(crate::notify::human::STOP, "Перегон USDT не состоялся")
                .line(format!(
                    "{} {}",
                    crate::notify::human::usd(amount),
                    crate::notify::human::route(&from.0, &to.0)
                ))
                .reason(&e.to_string())
                .todo(format!(
                    "деньги остались на {} — перегнать вручную",
                    crate::notify::human::venue(&from.0)
                ))
                .id(id)
                .render();
        deps.notify("usdt.relay_failed", Some(&from.0), &tg).await;
    }
    res
}

/// Перегон как relay, но сбой не извещается (решение 163: при повторах возврата выручки итог извещает
/// usdt_return_retry.rs — журнал на каждую попытку, Telegram только когда повторы кончились). Успех — журнал и
/// usdt.relayed. Возвращает: итог перегона (решение 236): ушло с площадки (после округления вывода), зачислено, газ
/// вывода не в USDT — в USDT (затраты возврата, deal_return_cost.rs). Ошибки: перегон не состоялся (OpError).
pub(super) async fn relay_try(
    core: &Core,
    id: &str,
    from: &VenueId,
    to: &VenueId,
    acc: Option<&DexAccountId>,
    amount: Decimal,
) -> Result<Relayed, OpError> {
    let deps = &core.inner.deps;
    // Решение 108: ожидание баланса у отправителя и повтор при откате (usdt_relay_retry.rs).
    let a = usdt(core, from)?;
    let sent_q = super::transfers::floored(core, from, &a, amount);
    let (sent, t) = super::usdt_relay_retry::send(core, id, from, to, acc, &a, amount).await?;
    // Решение 236: газ кошелька DEX (BNB) — в USDT по живой цене; комиссия в USDT уже в «ушло − зачислено».
    let (fee_usd, fee_note) =
        match super::deal_return_cost::fee_usd(core, from, &a.ticker, sent.fee.as_ref()).await {
            Ok(v) => (v, None),
            Err(e) => {
                let n = format!("газ возврата не пересчитан в USDT ({e}) — в итог не вошёл");
                deps.log(
                    Level::Warn,
                    "core.usdt_return",
                    Some(&from.0),
                    format!("{id}: {n}"),
                    None,
                )
                .await;
                (None, Some(n))
            }
        };
    let text = format!(
        "USDT {amount}: {} → {}, зачислено {}",
        from.0, to.0, t.amount
    );
    deps.log(Level::Info, "core.usdt_return", Some(&from.0), &text, None)
        .await;
    // Решение 164: текст Telegram — человеческий.
    let tg = crate::notify::human::Msg::new(crate::notify::human::OK, "USDT перегнаны")
        .line(format!(
            "{} {}",
            crate::notify::human::usd(amount),
            crate::notify::human::route(&from.0, &to.0)
        ))
        .kv("Зачислено", crate::notify::human::usd(t.amount))
        .id(id)
        .render();
    deps.notify("usdt.relayed", Some(&from.0), &tg).await;
    Ok(Relayed {
        sent: sent_q,
        credited: t.amount,
        fee_usd,
        fee_note,
    })
}

/// Режим «отдельно»: по свободным USDT площадок (из сверки). Решение 132: у DEX — каждый кошелёк отдельно; перегон
/// биржа ↔ DEX — только между биржей и её кошельком по маппингу, между кошельками DEX — нет.
/// Принимает: (площадка, счёт DEX) → свободно USDT.
/// Ошибок нет: не заданы пороги площадки — она пропускается (журнал пишет reconcile.rs); сухой режим — ничего.
pub async fn separate(core: &Core, free: &HashMap<Holder, Decimal>) {
    let s = &core.inner.deps.settings;
    if s.text("usdt_return.mode", At::default()).ok().as_deref() != Some("separate") {
        return;
    }
    // Сухой режим (решение 74): перегон без сделки — тоже перевод денег, не выполняется.
    if let Some(why) = super::dry::blocks(s) {
        tracing::info!("перегон USDT «отдельно» пропущен: {why}");
        return;
    }
    // Решение 118 (Я-13): свободное ведётся по ходу круга — донор не отдаёт дважды, получатель не становится донором
    // тех же денег; донор — только если после отдачи у него не меньше его порога (иначе перегон туда-обратно).
    let mut free = free.clone();
    let low_of = |h: &Holder| s.decimal("usdt_return.low_balance_usd", at(&h.0)).ok();
    let holders: Vec<Holder> = free.keys().cloned().collect();
    for low_h in &holders {
        let (Some(low), Ok(amount)) = (
            low_of(low_h),
            s.decimal("usdt_return.amount_usd", at(&low_h.0)),
        ) else {
            continue;
        };
        if free.get(low_h).is_none_or(|h| *h >= low) {
            continue;
        }
        let donor = free
            .iter()
            .filter(|(h, f)| *h != low_h && low_of(h).is_some_and(|l| **f - amount >= l))
            .filter(|(h, _)| pair_ok(core, h, low_h))
            // Решение 249: донор-биржа → биржа-получатель — по venue.cex_cex обеих и списку venue.cex_cex_deny донора.
            .filter(|(h, _)| {
                h.1.is_some()
                    || low_h.1.is_some()
                    || super::cex_link::pair(s, &h.0.0, &low_h.0.0).is_none()
            })
            .max_by_key(|(_, f)| **f)
            .map(|(h, _)| h.clone());
        if let Some(from) = donor {
            let id = format!("u{}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f"));
            let acc = from.1.clone().or_else(|| low_h.1.clone());
            // Итог перегона «отдельно» — в журнале и уведомлениях (relay); сверка продолжается.
            let _ = relay(core, &id, &from.0, &low_h.0, acc.as_ref(), amount).await;
            *free.entry(from).or_default() -= amount;
            *free.entry(low_h.clone()).or_default() += amount;
        }
    }
}

/// Можно ли перегнать между двумя держателями (решение 132): биржа ↔ биржа — да (решение 249 — ещё правила cex_link.rs
/// в separate); биржа ↔ кошелёк DEX — только если
/// кошелёк привязан к этой бирже в маппинге; кошелёк ↔ кошелёк — нет. Ошибок нет.
fn pair_ok(core: &Core, a: &Holder, b: &Holder) -> bool {
    let acc = core.inner.deps.settings.accounts();
    match (&a.1, &b.1) {
        (None, None) => true,
        (Some(w), None) => acc.row(&b.0.0).as_deref() == Some(w.0.as_str()),
        (None, Some(w)) => acc.row(&a.0.0).as_deref() == Some(w.0.as_str()),
        (Some(_), Some(_)) => false,
    }
}

/// Область настройки «площадка». Принимает: площадку. Возвращает: At. Ошибок нет.
fn at(v: &VenueId) -> At<'_> {
    At {
        venue: Some(&v.0),
        coin: None,
    }
}
