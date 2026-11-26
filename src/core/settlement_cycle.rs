//! Файл: core/settlement_cycle.rs.
//! Что делает: связывает фактическую покупку целевой монеты, её перевод, продажу и конверсию выручки.
//! Кто вызывает: исполнитель и восстановление ранее открытой сделки.
//! От чего зависит: доказанные количества ордеров, идентичность рынка, журнал до POST.
//! Настройки: допустимые цены исходного решения и актуальные сетевые ограничения.
use super::{
    deal::{Deal, Step},
    engine::Core,
    settlement_market::Market,
    settlement_state::{Leg, Plan, Progress},
    settlement_target as target,
    types::{DexAccountId, Direction, Side},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub const FIELD: &str = "settlement_cycle";
/// Пользовательская запись сделки содержит условия допуска, независимо от будущего состояния книги.
#[derive(Clone, Serialize, Deserialize)]
pub struct Mandate {
    pub target_cex: String,
    pub target_dex: String,
    pub accounts: super::settlement_accounts::Binding,
    pub market: Market,
    pub direction: Direction,
    pub wallet: DexAccountId,
    pub qty: Decimal,
    pub buy_limit: Decimal,
    pub sell_limit: Decimal,
    pub quote_cap: Decimal,
    pub leg: Leg,
}

impl Mandate {
    pub fn plan(&self, id: &str, leg: Leg) -> Plan {
        Plan {
            deal: id.into(),
            direction: self.direction.clone(),
            cex: self.market.cex.clone(),
            wallet: self.wallet.clone(),
            leg,
        }
    }
}

pub(super) async fn save(core: &Core, d: &Deal) -> Result<(), String> {
    super::persist::save(&core.inner.deps.db, super::ledger::KIND, &d.id, d)
        .await
        .map_err(|e| e.to_string())?;
    super::ledger::save(core, d).await;
    Ok(())
}

/// Результат одного прохода определяется сохранёнными ID. Каждая отправка предшествует только своей сверке.
pub async fn advance(core: &Core, deal: &mut Deal, p: &Mandate) -> Result<(), String> {
    let m = &p.market;
    let d = &p.direction;
    let wallet = &p.wallet;
    if p.target_cex != super::venue_ticker::ticker_on(core, &d.asset, &m.cex)
        || p.target_dex != super::venue_ticker::ticker_on(core, &d.asset, &m.dex)
    {
        return Err("Изменился тикер целевого контракта на исходной площадке".into());
    }
    if d != &deal.direction
        || deal.dex_account.as_ref() != Some(wallet)
        || !m.identical(&Market::direction(core, d)?)
    {
        return Err("Исходный рынок, сделка или привязанный кошелёк изменились".into());
    }
    let funding = d.buy_on == m.dex;
    if !super::settlement_unit::ratio(core, d)?.is_identity() {
        return Err("Для единиц целевой монеты нужен отдельный подтверждённый пересчёт".into());
    }
    let (wallet_cash, mut cash_move, mut cash_sender_remainder) =
        super::settlement_cycle_fund::prepare(core, deal, p, funding).await?;
    let buy = super::settlement_request::request(core, &deal.id, p, Side::Buy, p.qty, p.buy_limit)
        .await?;
    let buy =
        super::settlement_accounts::entry(target::order(core, m, wallet, buy, !funding)).await?;
    let (buy_debit, bought) = target::amounts(
        &buy,
        &buy.trade.request.asset,
        if funding { &m.asset.ticker } else { &m.quote },
    )?;
    if !funding {
        deal.invested = buy_debit;
    }
    deal.buy = Some(buy.trade.clone());
    deal.qty = bought;
    deal.at = d.buy_on.clone();
    deal.step = Step::Transfer;
    save(core, deal).await?;
    if bought <= Decimal::ZERO {
        return Err("Целевая покупка не исполнена".into());
    }
    if super::settlement_effect::previous_withdrawal(
        &core.inner.deps.db,
        m,
        &deal.id,
        &d.buy_on,
        &d.sell_on,
        Some(wallet),
    )
    .await
    .map_err(|e| e.message)?
    .is_none()
    {
        let exit = super::settlement_model::evaluate(core, d, bought).await?;
        if exit.sell_limit < p.sell_limit
            || exit.proceeds_quote - exit.other_costs_quote - exit.reserve_quote < deal.invested
        {
            return Err("После покупки фактический объём и текущий спрос не покрывают перевод и исходное вложение".into());
        }
    }
    let moving = super::settlement_transfer::move_token(
        core,
        &deal.id,
        (m, &d.asset),
        wallet,
        !funding,
        bought,
    )
    .await?;
    deal.transfers = vec![moving.sent.clone(), moving.credit.clone()];
    deal.at = d.sell_on.clone();
    deal.qty = moving.credit.amount;
    deal.step = Step::Selling;
    save(core, deal).await?;
    let sell = super::settlement_request::request(
        core,
        &deal.id,
        p,
        Side::Sell,
        moving.credit.amount,
        p.sell_limit,
    )
    .await?;
    let sell = target::order(core, m, wallet, sell, !funding).await?;
    let (disposed, received) = target::amounts(
        &sell,
        &sell.trade.request.asset,
        if funding { &m.quote } else { &m.asset.ticker },
    )?;
    deal.sells = vec![sell.trade.clone()];
    deal.qty = moving
        .credit
        .amount
        .checked_sub(disposed)
        .filter(|v| *v >= Decimal::ZERO)
        .ok_or("Продажа превысила кредит сделки")?;
    let mut remaining_cash = Decimal::ZERO;
    if funding {
        deal.proceeds = received;
        remaining_cash = wallet_cash
            .checked_sub(buy_debit)
            .filter(|v| *v >= Decimal::ZERO)
            .ok_or("Расход превысил фактическое финансирование")?;
    }
    save(core, deal).await?;
    if !funding && received > Decimal::ZERO {
        let Leg::Return { minimum_price, .. } = p.leg else {
            return Err("Нет исходной цены обратной конверсии".into());
        };
        let path = super::settlement_path::run(
            core,
            &p.plan(
                &deal.id,
                Leg::Return {
                    available_amount: received,
                    minimum_price,
                },
            ),
        )
        .await?;
        let Progress::Returned {
            credited_quote,
            held_cex_amount,
        } = path.progress()
        else {
            return Err("Ожидается возврат в исходной валюте биржи".into());
        };
        deal.proceeds = credited_quote;
        remaining_cash = held_cex_amount;
        if let Some(moving) = &path.movement {
            cash_sender_remainder =
                super::settlement_cycle_money::withdrawal_remainder(received, moving)?;
        }
        cash_move = path.movement;
    }
    let source_remainder = super::settlement_cycle_money::withdrawal_remainder(bought, &moving)?;
    deal.plan["settlement_residuals"] = serde_json::json!({"target_sender":source_remainder,"target_receiver":deal.qty,"cash_sender":cash_sender_remainder,"cash_receiver":remaining_cash,"cash_sender_venue":if funding{&m.cex}else{&m.dex},"cash_receiver_venue":if funding{&m.dex}else{&m.cex}});
    deal.spent =
        super::settlement_cycle_money::fees(core, p, &buy, &sell, &moving, cash_move.as_ref())
            .await?;
    if let Some(x) = cash_move {
        deal.transfers.extend([x.sent, x.credit]);
    }
    save(core, deal).await?;
    if deal.qty > Decimal::ZERO
        || source_remainder > Decimal::ZERO
        || remaining_cash > Decimal::ZERO
        || cash_sender_remainder > Decimal::ZERO
        || deal.proceeds <= Decimal::ZERO
    {
        return Err("Остатки сохранены в исходных валютах и местах; требуются новое решение по остаткам и сверка".into());
    }
    Ok(())
}
