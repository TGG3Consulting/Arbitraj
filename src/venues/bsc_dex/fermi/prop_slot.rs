//! Котировщик PropAMM и планировщик Ethereum проверяют модуль и строят поручение на один слот.
//! Симуляция зависит от канонического родителя, кадра выбранного сборщика и остатков плательщика.
use super::{Builder, Ctx, Frame, Head, canonical, slot};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    exec::prop_wallet,
    pools::{families::Family, prop_venue},
};
use alloy::primitives::{Address, B256, Bytes, I256, U64, U256};
use alloy::sol_types::SolCall;
use serde_json::json;
alloy::sol! {function balanceOf(address account) external view returns(uint256);}
/// Версия с защитой от повторного исполнения (completed[payer][leg]).
/// Источник — contracts/PropSlotVenueAdapter.sol, функция slotVersion().
const SINGLE_USE_SLOT_VERSION: u8 = 2;

/// По результату slotVersion() определяет, поддерживает ли модуль однократное поручение; ошибок нет.
fn supports_single_use(version: U256) -> bool {
    version == U256::from(SINGLE_USE_SLOT_VERSION)
}

/// По семейству, ролям и адресу модуля возвращает условия прямого исполнения на блоке h.
/// Ошибка RPC, несовпадение кода или ролей и неподдерживаемая версия исключают этот модуль.
pub(super) async fn terms(
    ctx: &Ctx,
    f: &Family,
    r: &prop_venue::Roles,
    module: Address,
    h: &Head,
) -> GatewayResult<prop_wallet::Terms> {
    let at = json!({"blockHash":h.hash,"requireCanonical":true});
    let (hash, version) = tokio::try_join!(
        prop_venue::module(ctx, f, r, module, &at),
        canonical(ctx, h.hash, module, slot::slotVersionCall {})
    )?;
    if !supports_single_use(version) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Модуль не поддерживает однократное поручение в слоте",
        ));
    }
    let mut t = r.terms(ctx, module, false)?;
    t.module_hash = Some(hash);
    Ok(t)
}
pub(super) struct Order {
    pub terms: prop_wallet::Terms,
    pub payer: Address,
    pub tokens: (Address, Address),
    pub specified: I256,
    pub paid: U256,
    pub minimum: U256,
    pub leg: B256,
}
impl Order {
    /// Из условий заказа, родительского блока и кадра сборщика формирует параметры вызова и восстановления.
    /// Сохраняет предел списания при покупке или минимум выплаты при продаже. Ошибок нет.
    pub fn proof(self, h: &Head, f: &Frame, builder: Builder) -> slot::Proof {
        let (input, output) = self.tokens;
        slot::Proof {
            builder: builder.name().into(),
            parent: h.hash,
            target: f.target,
            timestamp: f.timestamp,
            executor: self.terms.target,
            router: Some(self.terms.router),
            payer: self.payer,
            input,
            output,
            amount: self.specified,
            limit: if self.specified.is_negative() {
                self.paid
            } else {
                self.minimum
            },
            leg: self.leg,
            bop: None,
            prop: Some(Box::new(prop_wallet::Proof {
                terms: self.terms,
                payer: self.payer,
                input,
                output,
                amount: self.paid,
                minimum: self.minimum,
                deadline: U256::from(f.timestamp),
            })),
        }
    }
}
/// Исполняет сохранённый вызов в симуляции кадра f поверх блока h с указанными при наличии ставками газа.
/// Возвращает фактическое списание, выплату и расход газа. Ошибки RPC, откат, неверный ответ или
/// расхождение с дельтами кошелька либо пределами поручения возвращаются вызывающему.
pub(super) async fn simulate(
    ctx: &Ctx,
    h: &Head,
    f: &Frame,
    order: &slot::Proof,
    fees: Option<(u128, u128)>,
) -> GatewayResult<(U256, U256, u64)> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "PropAMM: не подтверждены обе суммы кошелька в заданном слоте",
        )
    };
    let balance = Bytes::from(
        balanceOfCall {
            account: order.payer,
        }
        .abi_encode(),
    );
    let view = |token| json!({"from":order.payer,"to":token,"data":balance});
    let mut trade = json!({"from":order.payer,"to":order.executor,"data":order.calldata()});
    if let Some((cap, tip)) = fees {
        trade["maxFeePerGas"] = json!(U256::from(cap));
        trade["maxPriorityFeePerGas"] = json!(U256::from(tip));
    }
    let rows = slot::run(
        ctx,
        h,
        f,
        json!([
            view(order.input),
            view(order.output),
            trade,
            view(order.input),
            view(order.output)
        ]),
    )
    .await?;
    let [a, b, swap, c, d] = rows.as_slice() else {
        return Err(bad());
    };
    if rows.iter().any(|v| v.status != U64::from(1)) || swap.gas_used.is_zero() {
        return Err(bad());
    }
    let read =
        |x: &super::SimCall| balanceOfCall::abi_decode_returns(&x.return_data).map_err(|_| bad());
    let spent = read(a)?.checked_sub(read(c)?).ok_or_else(bad)?;
    let received = read(d)?.checked_sub(read(b)?).ok_or_else(bad)?;
    let v = slot::executeAtSlotCall::abi_decode_returns(&swap.return_data).map_err(|_| bad())?;
    if (v.spent, v.received) != (spent, received) || !order.crossed(spent, received) {
        return Err(bad());
    }
    Ok((spent, received, swap.gas_used.to()))
}

/// Сравнивает реальные цены точного входа. Для покупки проверяет и соседние измеренные бюджеты;
/// точное количество осталось нижней границей выплаты, весь расход относится к требуемой позиции.
pub(super) async fn candidate(
    ctx: &Ctx,
    c: &super::indicative::Candidate,
    t: super::prop_levels::Quoted<'_>,
    leg: B256,
) -> Option<slot::Proof> {
    let b = &c.book;
    let buy = c.amount.is_negative();
    let wanted = c.amount.unsigned_abs();
    let mut sizes = std::collections::BTreeSet::new();
    if buy {
        let levels = b.sides.get(&(c.output, true))?;
        let estimated = levels.price(c.quote.qty, true)?;
        let raw = super::super::num::to_units(estimated, c.decs.1)?;
        sizes.insert(raw);
        // Между двумя измеренными выплатами запросим также цену промежуточного бюджета.
        // Линейная точка служит только кандидатом: минимальная выплата подтверждается отдельным вызовом.
        let prior = levels.0.iter().rev().find(|(qty, _)| *qty < c.quote.qty);
        let next = levels.0.iter().find(|(qty, _)| *qty >= c.quote.qty);
        if let (Some((low, lo_cash)), Some((high, hi_cash))) = (prior, next)
            && let Some(point) = hi_cash
                .checked_sub(*lo_cash)
                .and_then(|spread| spread.checked_mul(c.quote.qty - *low))
                .and_then(|part| part.checked_div(*high - *low))
                .and_then(|delta| lo_cash.checked_add(delta))
            && let Some(units) = super::super::num::to_units(point, c.decs.1)
        {
            sizes.insert(units);
            if let Some(up) = units.checked_add(U256::ONE) {
                sizes.insert(up);
            }
        }
        if let Some(up) = raw.checked_add(U256::ONE) {
            sizes.insert(up);
        }
        for (qty, cash) in &levels.0 {
            if *qty >= c.quote.qty
                && let Some(v) = super::super::num::to_units(*cash, c.decs.1)
            {
                sizes.insert(v);
            }
        }
    } else {
        sizes.insert(wanted);
    }
    let rows = super::prop_levels::quotes(
        ctx,
        &b.head,
        &b.frame,
        t,
        c.input,
        c.output,
        &sizes.into_iter().collect::<Vec<_>>(),
    )
    .await
    .ok()?;
    if let super::prop_levels::Quoted::Bop(path) = t {
        // Every preliminary amount is an exact-input estimate. Lock the wallet's observed terminal net.
        for (spent, _) in rows {
            let floor = if buy { wanted } else { U256::ONE };
            let probe = path.proof(c, leg, spent, floor);
            if let Ok((_, net, _)) = simulate(ctx, &b.head, &b.frame, &probe, None).await {
                return Some(path.proof(c, leg, spent, if buy { wanted } else { net }));
            }
        }
        return None;
    }
    let super::prop_levels::Quoted::Prop(t) = t else {
        return None;
    };
    let (spent, output) = rows.into_iter().find(|(_, out)| !buy || *out >= wanted)?;
    Some(
        Order {
            terms: t,
            payer: c.who,
            tokens: (c.input, c.output),
            specified: c.amount,
            paid: spent,
            minimum: if buy { wanted } else { output },
            leg,
        }
        .proof(&b.head, &b.frame, b.config.builder),
    )
}

#[cfg(test)]
#[path = "prop_slot_version_tests.rs"]
mod version_tests;
