//! Сохранённое плечо конверсии/перевода на исходную биржу или с неё.
use super::{
    settlement,
    settlement_cash::{self, CashFill},
    settlement_transfer::{self, Movement},
};
use crate::core::{
    engine::Core,
    types::{DexAccountId, Direction, Side, VenueId},
};
use crate::storage::db::Db;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

const KIND: &str = "base_settlement_path";

/// Вход для выдачи средств на Base. Порог потребности в USDC относится к полному объёму покупки токена.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Leg {
    Fund {
        budget_usdt: Decimal,
        buy_usdc: Decimal,
        maximum_price: Decimal,
        required_wallet_usdc: Decimal,
        target_qty: Decimal,
        sell_qty: Decimal,
        minimum_target_bid: Decimal,
    },
    Return {
        available_usdc: Decimal,
        minimum_price: Decimal,
    },
}

/// Начальное поручение сохраняется до любого движения денег, включая исходную биржу и привязанный кошелёк.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub deal: String,
    pub direction: Direction,
    pub cex: VenueId,
    pub wallet: DexAccountId,
    pub leg: Leg,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Path {
    pub plan: Plan,
    pub conversion: Option<CashFill>,
    pub movement: Option<Movement>,
}
impl Path {
    fn steps_confirmed(&self) -> bool {
        self.conversion.is_some() && self.movement.is_some()
    }
}
/// Сверка фактически достигнутого денежного результата. Остаток USDC после частичной продажи
/// остаётся на бирже. Недостаточный кредит на кошельке не разрешает первоначальную покупку.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Progress {
    Waiting,
    Funded {
        debited_usdt: Decimal,
        credited_usdc: Decimal,
        unspent_usdt: Decimal,
    },
    InsufficientFunding {
        debited_usdt: Decimal,
        held_wallet_usdc: Decimal,
    },
    Returned {
        credited_usdt: Decimal,
        held_cex_usdc: Decimal,
    },
}
impl Path {
    pub fn progress(&self) -> Progress {
        let (Some(c), Some(t)) = (&self.conversion, &self.movement) else {
            return Progress::Waiting;
        };
        match self.plan.leg {
            Leg::Fund {
                required_wallet_usdc,
                ..
            } if t.credit.amount < required_wallet_usdc => Progress::InsufficientFunding {
                debited_usdt: c.debit,
                held_wallet_usdc: t.credit.amount,
            },
            Leg::Fund { .. } => Progress::Funded {
                debited_usdt: c.debit,
                credited_usdc: t.credit.amount,
                unspent_usdt: c.unspent,
            },
            Leg::Return { .. } => Progress::Returned {
                credited_usdt: c.credit,
                held_cex_usdc: c.unspent,
            },
        }
    }
}

fn key(p: &Plan) -> Result<String, String> {
    serde_json::to_string(&(&p.deal, &p.direction)).map_err(|e| e.to_string())
}

/// Публикует одну версию пути. Конкурентный процесс продолжает с выигравшей версии на следующем входе.
async fn save(db: &Db, path: &Path, prev: Option<&Path>) -> Result<(), String> {
    let id = key(&path.plan)?;
    let body = serde_json::to_string(path).map_err(|e| e.to_string())?;
    let previous = prev
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| e.to_string())?;
    let n=db.ledger().write(move |c| {
        let now=Utc::now().to_rfc3339();
        match previous {
            None=> c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO NOTHING",params![KIND,id,body,now]),
            Some(previous)=> c.execute("UPDATE core_state SET body=?1,updated_at=?2 WHERE kind=?3 AND id=?4 AND body=?5",params![body,now,KIND,id,previous]),
        }
    }).await.map_err(|e|e.to_string())?;
    if n == 1 {
        Ok(())
    } else {
        Err("Состояние денежного пути уже изменилось; требуется чтение сохранённой версии".into())
    }
}

pub async fn get(db: &Db, p: &Plan) -> Result<Option<Path>, String> {
    let id = key(p)?;
    let body = db
        .ledger()
        .read(move |c| {
            c.query_row(
                "SELECT body FROM core_state WHERE kind=?1 AND id=?2",
                params![KIND, id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .await
        .map_err(|e| e.to_string())?;
    body.map(|v| serde_json::from_str(&v))
        .transpose()
        .map_err(|e| e.to_string())
}

pub(super) fn funding_wire(
    net: &crate::core::types::CoinNetworkInfo,
    amount: Decimal,
) -> Result<Decimal, String> {
    if net.use_net_withdrawal_budget {
        return super::withdraw_amount::amounts(amount, net)
            .map(|v| v.wire)
            .map_err(|e| e.to_string());
    }
    let sent = super::costs_withdraw::floor_precision(amount, Some(net));
    if net.withdraw_min.is_none_or(|x| sent < x) || net.withdraw_max.is_some_and(|x| sent > x) {
        return Err("Расчётный USDC вне подтверждённых пределов вывода".into());
    }
    let fee = net
        .fee_for(sent)
        .filter(|f| *f >= Decimal::ZERO)
        .ok_or("Нет комиссии вывода USDC")?;
    sent.checked_sub(fee)
        .filter(|x| *x > Decimal::ZERO)
        .ok_or_else(|| "Комиссия исчерпывает сумму вывода".into())
}

/// Свежая доступность обеих сторон монеты до финансирования, с ценой спроса в USDT исходной биржи.
async fn demand(
    core: &Core,
    plan: &Plan,
    target: Decimal,
    sell_qty: Decimal,
    bid: Decimal,
    required: Decimal,
) -> Result<(), String> {
    use super::costs_quote::{fetch, worst};
    let d = &plan.direction;
    let (a, b) = tokio::try_join!(
        fetch(core, d, false, target, None),
        fetch(core, d, true, sell_qty, None)
    )
    .map_err(|e| e.to_string())?;
    if target <= Decimal::ZERO
        || sell_qty <= Decimal::ZERO
        || bid <= Decimal::ZERO
        || required <= Decimal::ZERO
        || a.fillable_qty < target
        || b.fillable_qty < sell_qty
    {
        return Err("Нет глубины обеих сторон основной сделки Base".into());
    }
    let ask = worst(
        core,
        &d.buy_on,
        &d.asset,
        Side::Buy,
        target,
        a.avg_price,
        None,
    )
    .map_err(|e| e.to_string())?;
    let sale = worst(
        core,
        &d.sell_on,
        &d.asset,
        Side::Sell,
        sell_qty,
        b.avg_price,
        None,
    )
    .map_err(|e| e.to_string())?;
    if sale < bid || target.checked_mul(ask).is_none_or(|x| x > required) {
        return Err(
            "Основная сделка Base вышла за подтверждённые границы спроса или средств".into(),
        );
    }
    Ok(())
}

/// Доводит принятую часть маршрута до подтверждённого зачисления/конверсии. Нулевой или частичный
/// результат сохраняет остатки. Следующий ордер на остаток требует отдельного экономического решения.
pub async fn run(core: &Core, p: &Plan) -> Result<Path, String> {
    let db = &core.inner.deps.db;
    let dex = VenueId("base_dex".into());
    let funding = matches!(p.leg, Leg::Fund { .. });
    if p.deal.is_empty()
        || !p
            .direction
            .asset
            .chain
            .as_ref()
            .is_some_and(|c| c.as_str() == "base")
        || (if funding {
            (&p.direction.buy_on, &p.direction.sell_on)
        } else {
            (&p.direction.sell_on, &p.direction.buy_on)
        }) != (&dex, &p.cex)
    {
        return Err("Исходная биржа, направление или сеть не совпадают с денежным путём".into());
    }
    let mut path = match get(db, p).await? {
        Some(v) if v.plan == *p => v,
        Some(_) => return Err("Сохранённое поручение Base отличается от переданного".into()),
        None => {
            let v = Path {
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
            budget_usdt,
            buy_usdc,
            maximum_price,
            required_wallet_usdc,
            target_qty,
            sell_qty,
            minimum_target_bid,
        } => {
            if path.conversion.is_none() {
                if super::settlement_effect::previous_order(db, &p.deal, &p.cex, Side::Buy)
                    .await
                    .map_err(|e| e.message)?
                    .is_none()
                {
                    let net = settlement::usdc_network(core, &p.cex, true)?;
                    let offer = settlement::preview(core, &p.cex, Side::Buy, buy_usdc).await?;
                    if funding_wire(&net, offer.net_usdc)? < required_wallet_usdc {
                        return Err("План конверсии после комиссии вывода не финансирует требуемый объём покупки".into());
                    }
                    demand(
                        core,
                        p,
                        target_qty,
                        sell_qty,
                        minimum_target_bid,
                        required_wallet_usdc,
                    )
                    .await?;
                }
                let fill = settlement_cash::convert(
                    core,
                    &p.deal,
                    &p.cex,
                    Side::Buy,
                    buy_usdc,
                    budget_usdt,
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
                .ok_or("Нет исполнения покупки USDC")?
                .credit;
            if super::settlement_effect::previous_withdrawal(
                db,
                &p.deal,
                &p.cex,
                &dex,
                Some(&p.wallet),
            )
            .await
            .map_err(|e| e.message)?
            .is_none()
            {
                let net = settlement::usdc_network(core, &p.cex, true)?;
                let wire = funding_wire(&net, amount)?;
                if wire < required_wallet_usdc {
                    return Err("Купленный USDC после комиссии вывода не покрывает покупку на Base; остаток на бирже".into());
                }
                demand(
                    core,
                    p,
                    target_qty,
                    sell_qty,
                    minimum_target_bid,
                    required_wallet_usdc,
                )
                .await?;
            }
            path.movement = Some(
                settlement_transfer::move_usdc(core, &p.deal, &p.cex, &p.wallet, true, amount)
                    .await?,
            );
            save(db, &path, Some(&previous)).await?;
        }
        Leg::Return {
            available_usdc,
            minimum_price,
        } => {
            if path.movement.is_none() {
                if super::settlement_effect::previous_withdrawal(
                    db,
                    &p.deal,
                    &dex,
                    &p.cex,
                    Some(&p.wallet),
                )
                .await
                .map_err(|e| e.message)?
                .is_none()
                {
                    let q = settlement::sell_within(core, &p.cex, available_usdc).await?;
                    if q.limit_price < minimum_price || q.net_usdc > available_usdc {
                        return Err("Нет подтверждённого спроса в пределах остатка USDC".into());
                    }
                }
                path.movement = Some(
                    settlement_transfer::move_usdc(
                        core,
                        &p.deal,
                        &p.cex,
                        &p.wallet,
                        false,
                        available_usdc,
                    )
                    .await?,
                );
                save(db, &path, Some(&original)).await?;
            }
            let previous = path.clone();
            let credited = path
                .movement
                .as_ref()
                .ok_or("Нет зачисления USDC")?
                .credit
                .amount;
            path.conversion = Some(
                settlement_cash::convert(
                    core,
                    &p.deal,
                    &p.cex,
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

/// Сохранённые шаги одной сделки для экрана сверки и восстановления после остановки.
pub async fn rows(db: &Db, deal: Option<&str>) -> Result<Vec<Path>, String> {
    let deal = deal.map(String::from);
    let bodies=db.ledger().read(move |c| {
        let mut q=c.prepare("SELECT body FROM core_state WHERE kind=?1 AND ((?2 IS NULL AND (json_extract(body,'$.conversion') IS NULL OR json_extract(body,'$.movement') IS NULL)) OR json_extract(body,'$.plan.deal')=?2)")?;
        q.query_map(params![KIND,deal], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
    }).await.map_err(|e|e.to_string())?;
    bodies
        .into_iter()
        .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .collect()
}

/// После рестарта продолжает только ранее сохранённые поручения. Сначала выполняется сверка
/// уже отправленных операций. Перед первой отправкой следующего шага снова проверяются рынок и выключатели.
pub async fn resume(core: &Core) {
    use crate::gateway::urgency::{self, Urgency};
    use crate::storage::journal::Level;
    let paths = match rows(&core.inner.deps.db, None).await {
        Ok(v) => v,
        Err(e) => {
            core.inner
                .deps
                .log(Level::Error, "core.base_settlement_storage", None, &e, None)
                .await;
            return;
        }
    };
    for path in paths.into_iter().filter(|p| !p.steps_confirmed()) {
        let c = core.clone();
        tokio::spawn(urgency::scope(Urgency::Trade, async move {
            let mut repeats = 0;
            loop {
                match Box::pin(run(&c, &path.plan)).await {
                    Ok(_) => break,
                    Err(e) => {
                        c.inner
                            .deps
                            .log(
                                Level::Warn,
                                "core.base_settlement_pending",
                                Some(&path.plan.cex.0),
                                &format!("{}: {e}", path.plan.deal),
                                None,
                            )
                            .await
                    }
                }
                let Some(retry) = super::usdt_return_retry::plan(&c, &path.plan.cex) else {
                    break;
                };
                if repeats >= retry.max || retry.every.is_zero() {
                    break;
                }
                tokio::time::sleep(retry.every).await;
                repeats += 1;
            }
        }));
    }
}
