//! Исполнение кнопок из ответа на запрос цены. Котировка читает баланс и доли
//! без изменения учёта. Одно подтверждение даёт право продать только на выбранной площадке.

use rust_decimal::Decimal;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::remain_others::Own;
use crate::core::types::{DexAccountId, RecordKind, VenueId};
use crate::gateway::contract::VenueKind;

const STALE: &str = "Сделка или условия перевода изменились. Запросите цену ещё раз.";

pub(super) fn matches(expected: &Deal, current: &Deal) -> bool {
    matches!(current.step, Step::WaitingHuman | Step::Halted)
        && expected.id == current.id
        && expected.updated_at == current.updated_at
        && expected.step == current.step
        && expected.direction == current.direction
        && expected.at == current.at
        && expected.qty == current.qty
        && expected.dex_account == current.dex_account
        && expected.credit == current.credit
        && expected.proceeds == current.proceeds
        && expected.withdraw_refused == current.withdraw_refused
        && !current.sale_unsure
        && current.transit_to.is_none()
}

/// Свободная для исполнения доля сделки. Баланс блокируем на время чтения
/// тем же замком, что и распределение долей после зачисления. Ордера, выводы
/// и внутренние переводы между счетами этот путь не вызывает.
pub(super) async fn quote_qty(core: &Core, deal: &Deal) -> Result<Decimal, String> {
    if !matches!(deal.step, Step::WaitingHuman | Step::Halted)
        || deal.sale_unsure
        || deal.transit_to.is_some()
    {
        return Err("Сначала проверьте исход предыдущей операции по сделке.".into());
    }
    available_qty(core, deal, true).await
}

async fn available_qty(core: &Core, deal: &Deal, include_funding: bool) -> Result<Decimal, String> {
    let v = &deal.at;
    let asset = &deal.direction.asset;
    let _guard = super::credit_lock::fact(v, asset).await;
    // Обычная кнопка сначала распределяет спорный остаток. Здесь рассчитывается
    // та же доля по тому же спот-факту, пока не записывая её другим сделкам.
    let others = super::remain_others::on_account(core, deal, v);
    let rest_qty = if super::credit_fact::rest(deal).is_some() {
        let fact = super::remain_read::fact_at(core, deal, v).await?;
        Some(super::credit_rest_lock::preview(core, deal, fact, &others)?)
    } else {
        None
    };
    let want = rest_qty.unwrap_or(deal.qty);
    let (total, free) = if super::trade_ops::kind(core, v)? == VenueKind::Cex {
        let gw = super::dex_account::venue_of(core, deal, v).await?;
        let balance = gw
            .balance(&super::venue_ticker::balance_key(core, asset, v))
            .await
            .map_err(|e| format!("{}: баланс монеты: {}", v.0, e.message))?;
        let (total, free) = balance.map_or((Decimal::ZERO, Decimal::ZERO), |b| {
            (b.free + b.locked, b.free)
        });
        // Ручная продажа умеет добирать монеты со счёта роли ToTrading (решение 174).
        // На запрос цены лишь считаем сколько сможет перейти, без вызова move_to_trading.
        let movable = if include_funding && free < want {
            let on = super::venue_ticker::ticker_on(core, asset, v);
            match gw.account_balances().await {
                Ok(accounts) => super::to_trading::sources(gw.as_ref(), &accounts, &on)
                    .map(|(_, q)| q.max(Decimal::ZERO))
                    .sum::<Decimal>()
                    .min(want - free),
                Err(e) if free.is_zero() => {
                    return Err(format!("{}: счета монеты не прочитаны: {}", v.0, e.message));
                }
                Err(_) => Decimal::ZERO,
            }
        } else {
            Decimal::ZERO
        };
        (total + movable, free + movable)
    } else {
        let (free, _) = super::manual_halted::on_hand(core, deal, v, asset, Decimal::MAX).await?;
        (free, free)
    };
    let own = if let Some(q) = rest_qty {
        q
    } else {
        let input = super::credit_apply::input(core, deal, total, &others);
        let split = super::credit_split::split(&input);
        if super::credit_fact::pending(deal) {
            split.share_of(&deal.id).unwrap_or_default().min(deal.qty)
        } else {
            let others = super::credit_apply::with_shares(&others, &split);
            match super::remain_others::own_qty(deal.qty, total, &others) {
                Own::Qty(q) => q,
                Own::Human => return Err("Монеты на счёте принадлежат другим сделкам.".into()),
            }
        }
    };
    if own <= Decimal::ZERO || free < own {
        return Err(format!(
            "На {} доступно {free} {}, подтверждённая доля сделки {own}.",
            v.0, asset.ticker
        ));
    }
    Ok(own)
}

/// Кошелёк самой сделки (также у старых записей) либо кошелёк для первого перегона на DEX.
pub(super) fn wallet(
    core: &Core,
    deal: &Deal,
    to: &VenueId,
) -> Result<Option<DexAccountId>, String> {
    let source = super::dex_account::of(core, deal)?;
    if source.is_some() || to == &deal.at {
        Ok(source)
    } else {
        super::dex_account::for_relay(core, deal, to)
    }
}

fn route(
    core: &Core,
    deal: &Deal,
    to: &VenueId,
    wallet: Option<&DexAccountId>,
) -> Result<(), String> {
    if to != &deal.at {
        let (targets, _) = super::manual_relay_targets::targets(core, deal);
        if !targets.contains(to) {
            return Err(format!(
                "Перегон {} → {} сейчас недоступен. {STALE}",
                deal.at.0, to.0
            ));
        }
    }
    if self::wallet(core, deal, to)?.as_ref() != wallet {
        return Err(format!("Кошелёк маршрута изменился. {STALE}"));
    }
    if to != &deal.direction.buy_on
        && let Some(why) =
            super::usdt_return_deal::return_block(core, deal, to, &deal.direction.buy_on)
    {
        return Err(why);
    }
    Ok(())
}

/// Общий с обычными ручными командами захват записи. Под замком карты сделок проверяем её
/// версию и ставим Selling. Алерт в том же критическом участке забирается ровно один раз.
fn claim(core: &Core, kind: RecordKind, id: &str, expected: &Deal) -> Result<Deal, String> {
    let mut deals = lk(&core.inner.deals);
    let d = deals.get_mut(&expected.id).ok_or(STALE)?;
    if !matches(expected, d) {
        return Err(STALE.into());
    }
    match kind {
        RecordKind::Halted if id == d.id && d.step == Step::Halted => {}
        RecordKind::Alert if d.step == Step::WaitingHuman => {
            let mut alerts = lk(&core.inner.alerts);
            let (_, live) = alerts.get(id).ok_or(STALE)?;
            if !matches(expected, live) {
                return Err(STALE.into());
            }
            let (alert, _) = alerts.remove(id).ok_or(STALE)?;
            d.alert_was = Some(alert);
        }
        _ => return Err(STALE.into()),
    }
    d.step = Step::Selling;
    Ok(d.clone())
}

pub(super) async fn execute(
    core: &Core,
    kind: RecordKind,
    object_id: &str,
    expected: &Deal,
    quoted_qty: Decimal,
    destination: (&VenueId, Option<&DexAccountId>),
) -> Result<String, String> {
    let (to, wallet) = destination;
    if quoted_qty <= Decimal::ZERO {
        return Err(STALE.into());
    }
    route(core, expected, to, wallet)?;
    let key = CoinKey::of(&expected.direction.asset);
    if !super::coin_ride::take(core, &key) {
        return Err(
            "По монете выполняется другая операция. После её окончания запросите цену ещё раз."
                .into(),
        );
    }
    let mut deal = match claim(core, kind, object_id, expected) {
        Ok(d) => d,
        Err(e) => {
            super::coin_ride::free_active(core, &key);
            return Err(e);
        }
    };
    deal.return_usdt = true;
    deal.unsent_retries = 0;
    deal.clear_range();
    deal.note(format!(
        "По запросу цены выбрана продажа {quoted_qty} {} на {}; выручка — на {}.",
        deal.direction.asset.ticker, to.0, deal.direction.buy_on.0
    ));
    super::ledger::save(core, &deal).await;
    if kind == RecordKind::Alert {
        super::manual::persist(core).await;
    }
    let message = format!(
        "Сделка {}: принято решение продать на {}. Итог придёт отдельно.",
        deal.id, to.0
    );
    let (c, to, wallet) = (core.clone(), to.clone(), wallet.cloned());
    tokio::spawn(super::risk_count::manual(async move {
        Box::pin(sell_selected(&c, deal, quoted_qty, to, wallet)).await;
        super::coin_ride::free_active(&c, &key);
    }));
    Ok(message)
}

async fn sell_selected(
    core: &Core,
    mut deal: Deal,
    qty: Decimal,
    to: VenueId,
    wallet: Option<DexAccountId>,
) {
    use super::sell::BUTTON;
    if super::credit_fact::rest(&deal).is_some() {
        match Box::pin(super::credit_rest::for_button(core, &mut deal)).await {
            Ok(None) if deal.qty == qty => {}
            // for_button сообщил итог и закрыл фантом или передал его на разбор.
            Ok(Some(_)) => return,
            Ok(None) => return stop(core, deal, STALE).await,
            Err(e) => return stop(core, deal, &e).await,
        }
    }
    if super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex) {
        // Перекладка FUND → SPOT разрешена только теперь, после подтверждения.
        let hand =
            super::manual_halted::on_hand_sell(core, &deal, &deal.at, &deal.direction.asset, qty)
                .await;
        match hand {
            Ok((free, note)) if free >= qty => {
                if let Some(text) = note {
                    deal.note(text);
                }
            }
            Ok(_) => return stop(core, deal, STALE).await,
            Err(e) => return stop(core, deal, &e).await,
        }
    }
    // Биржа — штатная сверка с учётом чужих монет, как перед обычным перегоном.
    // У DEX эта сверка пропускается, поэтому проверяем кошелёк и долю перед отправкой сами.
    if !super::sell_fact::need(core, &deal) {
        match available_qty(core, &deal, false).await {
            Ok(free) if free == qty => super::manual_halted::set_qty(&mut deal, qty, None),
            Ok(_) => return stop(core, deal, STALE).await,
            Err(e) => return stop(core, deal, &e).await,
        }
    }
    let Some(mut deal) = Box::pin(super::sell_fact::before(core, deal, BUTTON)).await else {
        return;
    };
    if deal.qty != qty || super::credit_fact::rest(&deal).is_some() {
        return stop(core, deal, STALE).await;
    }
    if let Err(e) = route(core, &deal, &to, wallet.as_ref()) {
        return stop(core, deal, &e).await;
    }
    if deal.dex_account.is_none() {
        deal.dex_account = wallet;
    }
    if to == deal.at {
        return Box::pin(super::sell::market(core, deal, BUTTON)).await;
    }
    Box::pin(super::manual_relay::relay_to(core, deal, to, false)).await
}

pub(super) async fn stop(core: &Core, mut deal: Deal, why: &str) {
    super::ledger::finish(core, &mut deal, Step::Halted, why).await;
    super::manual_outcome::failed(core, &deal, super::sell::BUTTON, why).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Asset, Direction};

    #[test]
    fn a_confirmed_sale_must_still_match_the_waiting_position() {
        let origin = VenueId("origin".into());
        let direction = Direction {
            asset: Asset {
                ticker: "T".into(),
                contract: None,
                chain: None,
            },
            buy_on: origin.clone(),
            sell_on: VenueId("market".into()),
        };
        let mut original = Deal::new(
            "trade".into(),
            direction,
            serde_json::Value::Null,
            Decimal::ZERO,
        );
        original.step = Step::WaitingHuman;
        original.qty = Decimal::TEN;
        assert!(matches(&original, &original));
        let mut changed = original.clone();
        changed.qty -= Decimal::ONE;
        assert!(!matches(&original, &changed));
        let mut changed = original.clone();
        changed.step = Step::Selling;
        assert!(!matches(&original, &changed));
        let mut changed = original.clone();
        changed.sale_unsure = true;
        assert!(!matches(&original, &changed));
        let mut changed = original.clone();
        changed.transit_to = Some(origin);
        assert!(!matches(&original, &changed));
        let mut changed = original.clone();
        changed.dex_account = Some(DexAccountId("another".into()));
        assert!(!matches(&original, &changed));
        let mut changed = original.clone();
        changed.updated_at += chrono::Duration::seconds(1);
        assert!(!matches(&original, &changed));
    }
}
