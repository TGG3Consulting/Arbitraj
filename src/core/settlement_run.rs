//! Файл: core/settlement_run.rs.
//! Что делает: сохраняет допуск полного денежного маршрута и ведёт единственного исполнителя сделки.
//! Кто вызывает: обычный исполнитель после предпроверки и загрузка журнала при старте.
//! От чего зависит: маппинг, исходные реквизиты, лимит капитала и подтверждение каждой операции.
//! Настройки: пороги прибыли, лимиты сделки и интервал повторной сверки.
use super::{
    coin_key::CoinKey,
    costs::Calc,
    deal::{Deal, Step},
    engine::{Core, lk},
    settlement_accounts::Accounts,
    settlement_cycle::{self, FIELD, Mandate},
    settlement_market::Market,
    types::DexAccountId,
};
use crate::storage::journal::Level;

pub(super) fn pending(core: &Core, key: &CoinKey) -> bool {
    lk(&core.inner.deals).values().any(|d| {
        d.is_open() && d.plan.get(FIELD).is_some() && CoinKey::of(&d.direction.asset) == *key
    })
}

pub(super) async fn begin(
    core: &Core,
    m: &Market,
    plan: &Calc,
    wallet: &DexAccountId,
    decided: chrono::DateTime<chrono::Utc>,
    slot: Option<super::withdraw_limit::Slot>,
) -> Result<(), String> {
    let d = &plan.direction;
    m.ensure(core)?;
    super::executor::buy_gate(core)?;
    super::risk::can_open(core, d)?;
    let accounts = Accounts::read(core, m, wallet).await?;
    let model = crate::gateway::quote_stage::final_scope(super::settlement_model::evaluate(
        core, d, plan.qty,
    ))
    .await?;
    super::settlement_entry::thresholds(core, &model, plan.spend_max)?;
    if model.wallet != *wallet || model.buy_limit > plan.buy_worst {
        return Err("Кошелёк или предельная цена покупки изменились до открытия".into());
    }
    for (a, b) in [(&d.buy_on, &d.sell_on), (&d.sell_on, &d.buy_on)] {
        if let Some(e) = super::transfers::poll_missing(core, a, b) {
            return Err(e);
        }
    }
    let free = super::limits::free_now(core, &m.cex, None, &m.quote).await?;
    let held = super::usdt_hold::held(core, &m.cex, None).amount;
    if free
        .checked_sub(held)
        .is_none_or(|available| available < model.capital_quote)
    {
        return Err("Исходный свободный остаток после резерва меньше потребности круга".into());
    }
    let mandate = Mandate {
        market: m.clone(),
        direction: d.clone(),
        wallet: wallet.clone(),
        accounts: accounts.binding(),
        target_cex: super::venue_ticker::ticker_on(core, &d.asset, &m.cex),
        target_dex: super::venue_ticker::ticker_on(core, &d.asset, &m.dex),
        qty: plan.qty,
        buy_limit: model.buy_limit,
        sell_limit: model.sell_limit,
        quote_cap: model.capital_quote,
        leg: model.leg,
    };
    let mut data = serde_json::to_value(plan).map_err(|e| e.to_string())?;
    data[FIELD] = serde_json::to_value(&mandate).map_err(|e| e.to_string())?;
    let mut deal = {
        // Резерв одного исходного счёта и одного кошелька распределяется атомарно между монетами.
        static OPEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        let busy = lk(&core.inner.deals).values().any(|d| {
            d.is_open()
                && d.plan.get(FIELD).is_some_and(|v| {
                    v.pointer("/accounts/cex").and_then(|v| v.as_str())
                        == data[FIELD]
                            .pointer("/accounts/cex")
                            .and_then(|v| v.as_str())
                        || (v.pointer("/market/evm_chain_id")
                            == data[FIELD].pointer("/market/evm_chain_id")
                            && v.pointer("/accounts/wallet")
                                == data[FIELD].pointer("/accounts/wallet"))
                })
        });
        if busy {
            return Err("Ранее принятое поручение удерживает расчётный счёт или кошелёк".into());
        }
        super::ledger::open(core, d, data, model.profit_quote, Some(wallet.clone()))
    };
    deal.decided_at = Some(decided);
    deal.note(format!(
        "Расчёт через {} и {}, предел списания {} {}",
        m.asset.ticker, m.quote, mandate.quote_cap, m.quote
    ));
    if let Some(s) = slot {
        super::withdraw_limit::bind(core, &deal.id, s);
    }
    settlement_cycle::save(core, &deal).await?;
    follow(core, deal, &mandate).await
}

async fn follow(core: &Core, mut deal: Deal, p: &Mandate) -> Result<(), String> {
    let mut repeats = 0;
    loop {
        let outcome = async {
            if super::dex_account::of_direction(&core.inner.deps, &p.direction)
                .await?
                .as_ref()
                != Some(&p.wallet)
            {
                return Err("Изменился маппинг исходного кошелька".into());
            }
            let accounts = Accounts::read(core, &p.market, &p.wallet).await?;
            accounts
                .run(
                    core,
                    p,
                    Box::pin(settlement_cycle::advance(core, &mut deal, p)),
                )
                .await
        }
        .await;
        match outcome {
            Ok(()) => {
                if let Some(plan) = deal.plan.as_object_mut() {
                    plan.remove("settlement_pending");
                }
                super::ledger::finish(
                    core,
                    &mut deal,
                    Step::Closed,
                    "На исходной бирже подтверждена выручка в валюте учёта",
                )
                .await;
                return Ok(());
            }
            Err(reason) => {
                let text = format!("Сверка денежного маршрута {}: {reason}", deal.id);
                if deal.plan.get("settlement_pending").and_then(|v| v.as_str())
                    != Some(reason.as_str())
                {
                    deal.plan["settlement_pending"] = serde_json::Value::String(reason);
                    deal.note(text.clone());
                    settlement_cycle::save(core, &deal).await?;
                    core.inner
                        .deps
                        .log(
                            Level::Warn,
                            "core.settlement_pending",
                            Some(&p.market.cex.0),
                            &text,
                            None,
                        )
                        .await;
                }
                let Some(retry) = super::usdt_return_retry::plan(core, &p.market.cex) else {
                    return Ok(());
                };
                if repeats >= retry.max || retry.every.is_zero() {
                    return Ok(());
                }
                repeats += 1;
                tokio::time::sleep(retry.every).await;
            }
        }
    }
}

pub(super) fn resume(core: &Core) {
    let list: Vec<_> = lk(&core.inner.deals)
        .values()
        .filter(|d| d.is_open() && d.plan.get(FIELD).is_some())
        .cloned()
        .collect();
    for d in list {
        let c = core.clone();
        tokio::spawn(async move {
            let key = CoinKey::of(&d.direction.asset);
            if !super::coin_ride::take_settlement(&c, &key) {
                return;
            }
            let g = super::ride_guard::RideGuard {
                core: c,
                key,
                slot: super::ride_guard::Slot {
                    seat: super::coin_ride::Seat::Alone,
                    boarded: None,
                },
            };
            let res = match serde_json::from_value::<Mandate>(d.plan[FIELD].clone()) {
                Ok(p) => Box::pin(follow(&g.core, d, &p)).await,
                Err(e) => Err(format!(
                    "Не удалось прочитать сохранённое расчётное поручение: {e}"
                )),
            };
            if let Err(e) = res {
                g.core
                    .inner
                    .deps
                    .log(Level::Error, "core.settlement_pending", None, e, None)
                    .await;
            }
        });
    }
}
