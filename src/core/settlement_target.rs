//! Файл: core/settlement_target.rs.
//! Что делает: подтверждает один ордер основной монеты и обе фактические суммы его исполнения.
//! Кто вызывает: исполнитель денежного круга.
//! От чего зависит: подготовленный client ID биржи или подписанный хеш DEX, защищённый снимок рынка.
//! Настройки: цена и предел количества из подтверждённого решения.
use super::{
    engine::Core,
    settlement_market::Market,
    types::{Asset, DexAccountId, Side, TradeRequest, VenueId},
};
use crate::gateway::{
    contract::VenueKind,
    settlement_intent::{self as log, Entry, Outcome, Request, SpotFill},
};
use rust_decimal::Decimal;

/// Вход фиксируется один раз. После сбоя параметры запроса берутся из БД, включая исходный маршрут.
pub(super) async fn order(
    core: &Core,
    m: &Market,
    wallet: &DexAccountId,
    req: TradeRequest,
    from_cex: bool,
) -> Result<SpotFill, String> {
    let d = &core.inner.deps;
    let v = req.venue.clone();
    m.ensure(core)?;
    let token = req.asset.clone();
    if token.contract.is_none()
        || token.chain != m.asset.chain
        || token.contract == m.asset.contract
        || (v != m.dex && v != m.cex)
    {
        return Err("Целевая монета или площадка вне сохранённого направления".into());
    }
    let g = d.gw.get_as(&v, Some(wallet)).await.map_err(|e| e.message)?;
    let dex = g.kind() == VenueKind::Dex;
    let key = key(m, &req.deal_id, wallet, &v, req.side)?;
    let prior = log::load(&d.db, &key).await?;
    let fill = if let Some(entry) = prior {
        let Request::Order(stored) = entry.operation else {
            return Err("У шага сохранён вывод вместо ордера".into());
        };
        if stored.asset != token
            || stored.asset.ticker != token.ticker
            || stored.side != req.side
            || stored.venue != v
            || stored.deal_id != req.deal_id
        {
            return Err("Параметры сохранённого целевого ордера изменились".into());
        }
        let recorded = match entry.result {
            Some(Outcome::Order(done)) if !dex => return Ok(*done),
            Some(Outcome::Order(done)) => Some(done),
            Some(_) => return Err("Сохранён результат вывода на месте целевого ордера".into()),
            None => None,
        };
        let fresh = log::capture(async {
            let read = if dex {
                g.resolve_trade(*stored, &entry.lookup).await?
            } else {
                if !g.matches_settlement_order(&entry.lookup, &token.ticker, &m.quote) {
                    return Err(super::settlement_effect::uncertain(
                        &v,
                        "Сохранённый ордер относится к другой паре",
                    ));
                }
                g.inspect_settlement_order(*stored, &entry.lookup).await?
            };
            read.ok_or_else(|| {
                super::settlement_effect::uncertain(&v, "Исход целевого ордера ожидает сверки")
                    .pending(entry.lookup)
            })
        })
        .await
        .map_err(|e| e.message)?;
        if let Some(prior) = recorded
            && !same_execution(&prior, &fresh)
        {
            return Err(
                "Исходный ордер изменился при повторной проверке блока. Требуется сверка".into(),
            );
        }
        fresh
    } else {
        let check = m.guard(core, &token, from_cex);
        let market = m.clone();
        let peer = g.clone();
        let output = log::scope(&d.db, &key, Request::Order(Box::new(req.clone())), async {
            if dex {
                g.trade_settlement_swap(req, &m.asset).await
            } else {
                g.trade(req).await
            }
        });
        log::capture(log::checked(
            move |p| {
                check(p)?;
                if !dex && !peer.matches_settlement_order(p, &token.ticker, &market.quote) {
                    return Err(super::settlement_effect::uncertain(
                        &peer.id(),
                        "Подготовлена другая спотовая пара",
                    ));
                }
                Ok(())
            },
            output,
        ))
        .await
        .map_err(|e| e.message)?
    };
    if dex {
        let tx = fill
            .trade
            .chain
            .as_ref()
            .ok_or("Идентификатор транзакции отсутствует")?;
        crate::gateway::settlement_proofs::pin(&d.db,&v,m.evm_chain_id,&tx.hash.to_string()).await?;
    }
    match super::settlement_effect::commit(&d.db, &key, &v, Outcome::Order(Box::new(fill)))
        .await
        .map_err(|e| e.message)?
    {
        Outcome::Order(x) => Ok(*x),
        _ => Err("Несовместимое подтверждение целевого ордера".into()),
    }
}

fn same_execution(a: &SpotFill, b: &SpotFill) -> bool {
    a.trade.filled_qty == b.trade.filled_qty
        && a.gross_quote == b.gross_quote
        && a.trade.status == b.trade.status
        && a.trade.fees == b.trade.fees
        && a.trade
            .chain
            .as_ref()
            .zip(b.trade.chain.as_ref())
            .is_some_and(|(p, q)| {
                p.hash == q.hash
                    && p.nonce == q.nonce
                    && p.block == q.block
                    && p.gas_bnb == q.gas_bnb
                    && p.outcome == q.outcome
            })
}

fn key(
    m: &Market,
    deal: &str,
    wallet: &DexAccountId,
    v: &VenueId,
    side: Side,
) -> Result<String, String> {
    serde_json::to_string(&("target-order", deal, wallet, v, side, m)).map_err(|e| e.to_string())
}

/// Возвращает прежний ордер до расчёта новой цены либо баланса.
pub(super) async fn previous(
    core: &Core,
    m: &Market,
    deal: &str,
    wallet: &DexAccountId,
    v: &VenueId,
    side: Side,
) -> Result<Option<TradeRequest>, String> {
    match log::load(&core.inner.deps.db, &key(m, deal, wallet, v, side)?).await? {
        None => Ok(None),
        Some(Entry {
            operation: Request::Order(r),
            ..
        }) => Ok(Some(*r)),
        Some(_) => Err("У ордера сохранены несовместимые реквизиты".into()),
    }
}

/// Дебеты и кредиты обеих валют по исполнению. Газ и комиссии в третьей валюте оцениваются отдельно.
pub(super) fn amounts(
    f: &SpotFill,
    token: &Asset,
    quote: &str,
) -> Result<(Decimal, Decimal), String> {
    let t = &f.trade;
    if t.request.asset != *token
        || t.request.asset.ticker != token.ticker
        || t.filled_qty < Decimal::ZERO
        || (t.chain.is_none() && t.filled_qty > t.request.qty)
        || t.venue_ref.is_empty()
        || f.gross_quote < Decimal::ZERO
        || t.filled_qty.is_zero() != f.gross_quote.is_zero()
    {
        return Err("Некорректные стороны фактического целевого ордера".into());
    }
    let mut asset_fee = Decimal::ZERO;
    let mut quote_fee = Decimal::ZERO;
    if t.chain.is_none() {
        for fee in &t.fees {
            if fee.amount < Decimal::ZERO {
                return Err("Биржа вернула неоднозначную отрицательную комиссию".into());
            }
            let part = if fee.asset == token.ticker {
                &mut asset_fee
            } else if fee.asset == quote {
                &mut quote_fee
            } else {
                continue;
            };
            *part = part
                .checked_add(fee.amount)
                .ok_or("Комиссия вне диапазона")?;
        }
    }
    let (debit, credit) = match t.request.side {
        Side::Buy => (
            f.gross_quote.checked_add(quote_fee),
            t.filled_qty.checked_sub(asset_fee),
        ),
        Side::Sell => (
            t.filled_qty.checked_add(asset_fee),
            f.gross_quote.checked_sub(quote_fee),
        ),
    };
    let valid = |v: Option<Decimal>| {
        v.filter(|n| *n >= Decimal::ZERO)
            .ok_or("Недопустимая сумма исполнения".to_string())
    };
    Ok((valid(debit)?, valid(credit)?))
}

#[cfg(test)]
#[path = "settlement_target_tests.rs"]
mod tests;
