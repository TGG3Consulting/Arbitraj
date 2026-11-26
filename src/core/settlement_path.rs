//! Файл: core/settlement_path.rs.
//! Что делает: исполняет последовательность финансирования либо возврата после проверки рынка.
//! Кто вызывает: восстановление движка и исполнитель.
//! От чего зависит: сохранённое поручение, идентификаторы ордеров и выводов.
//! Настройки: выключатели площадок, dry.enabled и условия выбранной сети.
pub use super::settlement_recovery::{get, resume, rows};
pub use super::settlement_state::{Leg, Path, Plan, Progress};
use super::{
    engine::Core, settlement, settlement_cash, settlement_demand::demand,
    settlement_recovery::save, settlement_transfer, types::Side,
};
/// Доводит принятую часть маршрута до подтверждённого зачисления/конверсии. Нулевой или частичный
/// результат сохраняет остатки. Следующий ордер на остаток требует отдельного экономического решения.
pub async fn run(core: &Core, p: &Plan) -> Result<Path, String> {
    let db = &core.inner.deps.db;
    let market = super::settlement_market::Market::direction(core, &p.direction)?;
    market.accounting()?;
    let dex = market.dex.clone();
    if p.deal.is_empty()
        || p.cex != market.cex
        || matches!(p.leg, Leg::Fund { .. }) != (p.direction.buy_on == dex)
    {
        return Err("Исходная биржа, направление или сеть не совпадают с денежным путём".into());
    }
    let mut path = match get(db, p).await? {
        Some(v) if v.permits(p, &market) => v,
        Some(_) => return Err("Сохранённое поручение конверсии отличается от переданного".into()),
        None => {
            let v = Path {
                market: Some(market.clone()),
                plan: p.clone(),
                movement: None,
                conversion: None,
            };
            save(db, &v, None).await?;
            v
        }
    };
    if path.steps_confirmed() {
        return Ok(path);
    }
    let original = path.clone();
    match p.leg {
        Leg::Fund {
            quote_budget,
            buy_amount,
            maximum_price,
            required_wallet_amount,
            target_qty,
            sell_qty,
            minimum_target_bid,
        } => {
            if path.conversion.is_none() {
                if super::settlement_effect::previous_order(db, &market, &p.deal, &p.cex, Side::Buy)
                    .await
                    .map_err(|e| e.message)?
                    .is_none()
                {
                    let net = market.network(core, true)?;
                    let offer = settlement::preview(core, &market, Side::Buy, buy_amount).await?;
                    if super::settlement_funding::funding_wire(&net, offer.net_asset)?
                        < required_wallet_amount
                    {
                        return Err("План конверсии после комиссии вывода не финансирует требуемый объём покупки".into());
                    }
                    demand(
                        core,
                        p,
                        target_qty,
                        sell_qty,
                        minimum_target_bid,
                        required_wallet_amount,
                    )
                    .await?;
                }
                let fill = settlement_cash::convert(
                    core,
                    &p.deal,
                    &market,
                    Side::Buy,
                    buy_amount,
                    quote_budget,
                    maximum_price,
                )
                .await?;
                path.conversion = Some(fill);
                save(db, &path, Some(&original)).await?;
            }
            let previous = path.clone();
            let amount = path
                .conversion
                .as_ref()
                .ok_or("Нет исполнения покупки расчётного актива")?
                .credit;
            if super::settlement_effect::previous_withdrawal(
                db,
                &market,
                &p.deal,
                &p.cex,
                &dex,
                Some(&p.wallet),
            )
            .await
            .map_err(|e| e.message)?
            .is_none()
            {
                let net = market.network(core, true)?;
                let wire = super::settlement_funding::funding_wire(&net, amount)?;
                if wire < required_wallet_amount {
                    return Err("Купленный расчётного актива после комиссии вывода не покрывает покупку на конверсии; остаток на бирже".into());
                }
                demand(
                    core,
                    p,
                    target_qty,
                    sell_qty,
                    minimum_target_bid,
                    required_wallet_amount,
                )
                .await?;
            }
            path.movement = Some(
                settlement_transfer::move_asset(core, &p.deal, &market, &p.wallet, true, amount)
                    .await?,
            );
            save(db, &path, Some(&previous)).await?;
        }
        Leg::Return {
            available_amount,
            minimum_price,
        } => {
            if path.movement.is_none() {
                if super::settlement_effect::previous_withdrawal(
                    db,
                    &market,
                    &p.deal,
                    &dex,
                    &p.cex,
                    Some(&p.wallet),
                )
                .await
                .map_err(|e| e.message)?
                .is_none()
                {
                    let q = settlement::sell_within(core, &market, available_amount).await?;
                    if q.limit_price < minimum_price || q.net_asset > available_amount {
                        return Err(
                            "Нет подтверждённого спроса в пределах остатка расчётного актива"
                                .into(),
                        );
                    }
                }
                path.movement = Some(
                    settlement_transfer::move_asset(
                        core,
                        &p.deal,
                        &market,
                        &p.wallet,
                        false,
                        available_amount,
                    )
                    .await?,
                );
                save(db, &path, Some(&original)).await?;
            }
            let previous = path.clone();
            let credited = path
                .movement
                .as_ref()
                .ok_or("Нет зачисления расчётного актива")?
                .credit
                .amount;
            path.conversion = Some(
                settlement_cash::convert(
                    core,
                    &p.deal,
                    &market,
                    Side::Sell,
                    credited,
                    credited,
                    minimum_price,
                )
                .await?,
            );
            save(db, &path, Some(&previous)).await?;
        }
    }
    Ok(path)
}
