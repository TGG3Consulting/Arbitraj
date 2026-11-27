//! Симуляция продажи до покупки (решения 380, 407 и О-65). На BSC берёт
//! результат фоновой проверки баланса для своего кошелька на блоке котировки.
//! Если результат не годится, штатный dry_fund проверяет объём по источникам.
//! Вызывает exec/presell.rs. Настройка площадки — presell.balance_slot_scan.
mod prepare;
use alloy_primitives::Address;
use prepare::prepare;

use crate::gateway::errors::GatewayResult;

use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::quote::Quoting;
use super::dry_build::Ready;
use super::dry_sim::{self, Io, Sim};
use super::presell_slot_find as find;

/// Симуляция продажи до покупки (см. шапку). Принимает: котировку, сделку, кошелёк, вход (Io.credit и Io.have
/// строятся здесь). Возвращает: Sim (с пометкой решения 407, если подмена не применена). Ошибки: как у dry_sim::run
/// (и узел — баланс входа на блоке симуляции).
pub async fn run(q: &Quoting, r: &Ready, wallet: Address, io: &Io<'_>) -> GatewayResult<Sim> {
    let p = prepare(q, r, wallet, io).await?;
    let mut note = p.note;
    let with = Io {
        credit: p.credit,
        have: p.have,
        ..*io
    };
    let attempt = dry_sim::run_anchored(q, r, wallet, &with, p.stamp.map(|s| s.hash)).await;
    let mut sim = match attempt {
        Ok(value) => value,
        Err(e) if p.stamp.is_some() => {
            // Блок мог быть заменён. Сохраняем причину и повторяем без старого баланса.
            Sim {
                credit_bad: true,
                reason: Some(e.message),
                ..Default::default()
            }
        }
        Err(e) => return Err(e),
    };
    if sim.credit_bad {
        let why = sim.reason.take().unwrap_or_default();
        if let Some(t) = io.token_in {
            if let Some(stamp) = p.stamp {
                q.ctx.evm_chain.presell().reject(wallet, t, stamp, &why);
            } else {
                find::reject(q.ctx.place.id, t, p.n, &why);
            }
        }
        note = Some(format!("{why} — повтор без подмены (решение 407)"));
        let plain = Io {
            credit: None,
            have: if let (Some(_), Some(t)) = (p.stamp, io.token_in) {
                Some(
                    call::view(
                        &q.ctx,
                        t,
                        &IERC20::balanceOfCall { owner: wallet },
                        &call::block_tag(r.at.number),
                    )
                    .await?,
                )
            } else {
                p.have
            },
            ..*io
        };
        sim = dry_sim::run(q, r, wallet, &plain).await?;
    }
    // Пока dry_fund проверял источники, фон мог подготовить полный объём.
    // Не превращаем такой исход в урезание 380 и не ждём незавершённый поиск.
    if p.n > 0
        && p.stamp.is_none()
        && sim.fundable.is_some()
        && super::presell_warm::applies(&q.ctx)
        && let Some(t) = io.token_in
        && let Ok((stamp, field, have)) =
            q.ctx
                .evm_chain
                .presell()
                .read(wallet, t, r.at.number, q.ctx.settings.generation())
        && io.need <= field.max
    {
        let retry = Io {
            credit: if have < io.need {
                field.credit(io.need)
            } else {
                None
            },
            have: Some(have),
            ..*io
        };
        match dry_sim::run_anchored(q, r, wallet, &retry, Some(stamp.hash)).await {
            Ok(result) if !result.credit_bad => {
                q.ctx.evm_chain.presell().funding_short(true);
                sim = result;
                note=Some("фоновая проверка завершилась во время проверки источников; проверен полный объём".into());
            }
            Ok(result) => q.ctx.evm_chain.presell().reject(
                wallet,
                t,
                stamp,
                &result.reason.unwrap_or_default(),
            ),
            Err(e) => q
                .ctx
                .evm_chain
                .presell()
                .reject(wallet, t, stamp, &e.message),
        }
    }
    if p.warm && sim.fundable.is_some() {
        q.ctx.evm_chain.presell().funding_short(false);
    }
    if let Some(n) = note {
        sim.reason = sim.reason.map(|w| format!("{w}; {n}"));
        sim.funding = Some(match sim.funding.take() {
            Some(f) => format!("{f}; {n}"),
            None => n,
        });
    }
    Ok(sim)
}
