//! Файл: core/remain_read.rs
//! Что делает: решение 413 (вынесено из remain_fact.rs; повторное ревью t198, Б-3) — чтение факта счёта сделки для
//! сверки остатка и доли: баланс монеты (биржа — торговый и перекладываемые на торговый счета, free + locked; DEX —
//! кошелёк сделки) и учёт других сделок — одним снимком под замком счёта (credit_lock.rs). На бирже сбой чтения с кодом
//! шлюза повторяется по правилам шлюза для этого кода (rules_apply::on_error: «повторить» — пауза и снова; «стоп» —
//! ошибка наверх, remain_fact.rs решает дальше); сбой без кода (счёт не прочитан, нет шлюза) — сразу ошибка.
//! Кто вызывает: core/remain_fact.rs (reconcile), core/credit_base.rs и core/credit_apply.rs (fact_at).
//! От чего зависит: credit_lock.rs, remain_others.rs, rules_apply.rs, dex_account.rs, manual_halted.rs (free_of),
//! venue_ticker.rs, engine.rs.
//! Настройки: правила шлюза (через rules_apply.rs).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::contract::{AccountRole, VenueKind};
use crate::gateway::errors::GatewayError;

use super::deal::Deal;
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: факт счёта сделки и учёт других под замком, с повторами по правилам шлюза (шапка). Принимает: ядро,
/// сделку. Возвращает: (факт или текст ошибки, другие). Ошибок нет.
pub async fn read(core: &Core, deal: &Deal) -> (Result<Decimal, String>, Others) {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let g = super::credit_lock::fact(core, &deal.at, &deal.direction.asset).await;
        let r = Box::pin(fact_try(core, deal, &deal.at)).await;
        let mut o = super::remain_others::on_account(core, deal, &deal.at);
        o.busy = g.is_none();
        drop(g);
        match r {
            Ok(f) => return (Ok(f), o),
            Err((text, Some(ge))) => {
                let next = Box::pin(super::rules_apply::on_error(
                    core,
                    "Чтение баланса для доли сделки",
                    &deal.at,
                    Some(&deal.id),
                    &ge,
                    attempt,
                ))
                .await;
                match next {
                    super::rules_apply::Next::Retry { pause_ms } => {
                        tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await
                    }
                    super::rules_apply::Next::Stop => return (Err(text), o),
                }
            }
            Err((text, None)) => return (Err(text), o),
        }
    }
}

/// Фактическое количество монеты сделки на площадке `v` (и до вывода туда — credit_base.rs). Ошибки — текст.
pub async fn fact_at(core: &Core, deal: &Deal, v: &VenueId) -> Result<Decimal, String> {
    fact_try(core, deal, v).await.map_err(|e| e.0)
}

/// Что делает: факт на площадке; ошибка — текст и ошибка шлюза (если она с кодом). Ошибок нет, кроме возвращаемой.
async fn fact_try(
    core: &Core,
    deal: &Deal,
    v: &VenueId,
) -> Result<Decimal, (String, Option<GatewayError>)> {
    let a = &deal.direction.asset;
    let gw = super::dex_account::venue_of(core, deal, v)
        .await
        .map_err(|e| (e, None))?;
    if gw.kind() != VenueKind::Cex {
        return super::manual_halted::free_of(core, gw.as_ref(), v, a)
            .await
            .map_err(|e| (e, None));
    }
    let on = super::venue_ticker::ticker_on(core, a, v);
    let accs = gw.account_balances().await.map_err(|e| {
        let t = format!("{}: балансы счетов: {}", v.0, e.message);
        (t, Some(e))
    })?;
    let mut sum = Decimal::ZERO;
    for acc in accs
        .iter()
        .filter(|x| gw.account_role(&x.account) != AccountRole::Other)
    {
        if let Some(e) = &acc.error {
            let t = format!("{}: счёт «{}» не прочитан: {e}", v.0, acc.account);
            return Err((t, None));
        }
        sum += acc
            .balances
            .iter()
            .filter(|b| b.asset == on)
            .map(|b| b.free + b.locked)
            .sum::<Decimal>();
    }
    Ok(sum)
}
