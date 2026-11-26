//! Файл: core/settlement_cycle_fund.rs.
//! Что делает: проверяет потребность кошелька и даёт начать целевую покупку только на фактически доступные средства.
//! Кто вызывает: исполнитель расчётного круга.
//! От чего зависит: сохранённая конверсия, остатки исходной биржи и текущий спрос.
//! Настройки: пороги прибыли, резерв баланса и условия принятого поручения.
use super::{
    deal::Deal,
    engine::Core,
    settlement_cycle::{Mandate, save},
    settlement_state::Progress,
    settlement_target as target,
    settlement_transfer::Movement,
    types::Side,
};
use rust_decimal::Decimal;
pub(super) async fn prepare(
    core: &Core,
    deal: &mut Deal,
    p: &Mandate,
    funding: bool,
) -> Result<(Decimal, Option<Movement>, Decimal), String> {
    let m = &p.market;
    let d = &p.direction;
    let wallet = &p.wallet;
    let mut wallet_cash = Decimal::ZERO;
    let mut cash_move = None;
    let mut cash_sender_remainder = Decimal::ZERO;
    if funding {
        let plan = p.plan(&deal.id, p.leg.clone());
        if super::settlement_effect::previous_order(
            &core.inner.deps.db,
            m,
            &deal.id,
            &m.cex,
            Side::Buy,
        )
        .await
        .map_err(|e| e.message)?
        .is_none()
        {
            super::risk::can_open(core, d)?;
            let fresh = super::settlement_model::evaluate(core, d, p.qty).await?;
            super::settlement_entry::thresholds(core, &fresh, p.quote_cap)?;
            if fresh.sell_limit < p.sell_limit || fresh.buy_limit > p.buy_limit {
                return Err("Цена или спрос изменились до финансирования".into());
            }
        }
        let result =
            super::settlement_accounts::entry(super::settlement_path::run(core, &plan)).await;
        let recorded = super::settlement_path::get(&core.inner.deps.db, &plan).await?;
        if let Some(path) = recorded.as_ref().filter(|v| v.permits(&plan, m))
            && let Some(fill) = &path.conversion
        {
            deal.invested = fill.debit;
            deal.plan["settlement_funding"] = serde_json::json!({"spent":fill.debit,"currency":m.quote,"bought":fill.credit,"asset":m.asset,"credited_wallet":path.movement.as_ref().map(|v|v.credit.amount)});
            save(core, deal).await?;
        }
        let path = result?;
        let Progress::Funded {
            debited_quote,
            credited_asset,
            ..
        } = path.progress()
        else {
            return Err(
                "Остаток финансирования удержан в записанной валюте и на своей площадке".into(),
            );
        };
        deal.invested = debited_quote;
        wallet_cash = credited_asset;
        if let (Some(c), Some(moving)) = (&path.conversion, &path.movement) {
            cash_sender_remainder =
                super::settlement_cycle_money::withdrawal_remainder(c.credit, moving)?;
        }
        cash_move = path.movement;
        save(core, deal).await?;
    }
    let need_request = target::previous(core, m, &deal.id, wallet, &d.buy_on, Side::Buy)
        .await?
        .is_none();
    if need_request {
        super::risk::can_open(core, d)?;
        let fresh = super::settlement_model::evaluate(core, d, p.qty).await?;
        super::settlement_entry::thresholds(core, &fresh, p.quote_cap)?;
        let required = p
            .qty
            .checked_mul(fresh.buy_limit)
            .ok_or("Расход покупки вне диапазона")?;
        let expected = if funding {
            super::venue_ticker::balance_key(core, &m.asset, &m.dex)
        } else {
            m.quote.clone()
        };
        let free = super::limits::free_now(core, &d.buy_on, Some(wallet), &expected).await?;
        let free = if funding {
            free
        } else {
            super::usdt_hold::spendable(
                free,
                &super::usdt_hold::held_except(core, &d.buy_on, None, Some(&deal.id)),
            )
        };
        let bound = if funding { wallet_cash } else { p.quote_cap };
        if required > bound
            || required > free
            || fresh.buy_limit > p.buy_limit
            || fresh.sell_limit < p.sell_limit
        {
            return Err("Средств или текущего спроса недостаточно для целевой покупки".into());
        }
    }
    Ok((wallet_cash, cash_move, cash_sender_remainder))
}
