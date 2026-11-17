//! Файл: venues/bsc_dex/exec/dry_fund.rs
//! Что делает: вход сухой сделки DEX (решение 121, C4), когда у кошелька его не хватает: тот же приём, что у пробы
//! налогов (check/probe.rs, check/service_tax_calls.rs) — первым вызовом той же симуляции перевод токена на кошелёк от
//! имени пула-источника (eth_simulateV1 с validation = false не требует подписи отправителя), без state override.
//! Источник — пул V2/V3 реестра (адрес контракта держит свои токены), достоверный, НЕ из маршрута сделки (иначе
//! перевод сдвинул бы его баланс против резервов), с наибольшим балансом этого токена ≥ суммы. Сумма перевода —
//! недостача, увеличенная на налог токена (наибольший из налога покупки и перевода — перевод от пула токен может
//! считать покупкой), чтобы после налога у кошелька было не меньше входа; что реально дошло — меряет симуляция.
//! Решение 302 (задача t133): пула-источника нет и задана presell.fund_fallback = singleton — запасной источник,
//! держатель-синглтон Uniswap V4 PoolManager / Infinity Vault (exec/dry_fund_holder.rs, там же — почему это честно и
//! когда держатель не берётся). Не задана — как до решения (тот же отказ тем же текстом).
//! Решение 380 (задача t166): одного источника с полной суммой нет — несколько источников вместе (пулы вне маршрута и
//! годные держатели) через «узел» — один перевод полной суммы на кошелёк (ревью t166, Н4); и вместе мало — Short с
//! объёмом, который можно проверить (exec/dry_fund_gather.rs; ядро пересчитывает сделку на него —
//! core/precheck_dex_cut.rs). Ревью t166: держатели читаются только если одного пула мало (Б1); балансы — на блоке
//! симуляции r.at (Н1); запас на округления в проверяемом объёме (Н5).
//! Кто вызывает: exec/dry_sim.rs (plan).
//! От чего зависит: pools/registry.rs, node/call.rs (view_many: Multicall3 / пачка), node/abi.rs (IERC20),
//! exec/dry_build.rs (entries — пулы маршрута), exec/dry_fund_holder.rs, exec/dry_fund_gather.rs.
//! Настройки: presell.fund_fallback [bsc_dex] (через dry_fund_holder; через вызываемые — dex.multicall3, dex.vault).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::GatewayResult;

use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::quote::Quoting;
use super::dry_build::{Ready, entries};
use super::dry_fund_gather::{self as gather, Pick};
use super::dry_fund_holder;

/// Пополнение (перевод внутри симуляции) — exec/dry_fund_gather.rs (там же текст для журнала).
pub use super::dry_fund_gather::{Fund, text};

/// Пополнить на полную сумму нечем (решение 380): причина для журнала и сколько единиц входа МОЖНО дать кошельку
/// (его баланс + всё, что дойдёт от всех источников после налога, минус запас на округления) — по нему считается
/// объём, который можно проверить.
pub struct Short {
    pub why: String,
    pub fundable: U256,
}

/// Пополнение кошелька в симуляции: переводы. Принимает: котировку, сделку (пулы маршрута исключаются; балансы
/// источников — на её блоке r.at, на нём же идёт симуляция — ревью t166 Н1), токен, (есть у кошелька, нужно), налоги
/// токена. Возвращает: Ok(Ok(переводы)) — один источник (как до решения 380) или узел из нескольких
/// (exec/dry_fund_gather.rs Pick::Relay); Ok(Err(Short)) — набрать нельзя. Держатели (presell.fund_fallback)
/// читаются только если одного пула с полной суммой нет (ревью t166 Б1: обычная сделка — без лишнего круга до узла);
/// ошибка их чтения — причина в тексте, не обрыв. Ошибки: узел (balanceOf пулов), Internal (настройки).
pub async fn plan(
    q: &Quoting,
    r: &Ready,
    token: Address,
    (have, need): (U256, U256),
    taxes: Option<&TokenTaxes>,
) -> GatewayResult<Result<Vec<Fund>, Short>> {
    let legs = entries(&r.legs);
    let route: Vec<_> = legs.iter().map(|e| e.pool.reference.clone()).collect();
    // Ревью t166 Н5: запас на округления — по единице на долю продажи (сумма долей новой сделки) и на перевод.
    let margin = |n: usize| U256::from(r.legs.len() + n);
    let Some(amount) = gross(need.saturating_sub(have), taxes) else {
        return Ok(Err(Short {
            why: "налог токена входа не позволяет посчитать пополнение".into(),
            fundable: have.saturating_sub(margin(0)),
        }));
    };
    let at = call::block_tag(r.at.number);
    let pools = sources(q, token, &route, &at).await?;
    let pool = |src| Fund {
        src,
        to: None,
        amount,
        what: "пула".into(),
    };
    // Обычный случай — один пул с полной суммой: держатели не читаются (как до решения 380).
    if let Ok(Pick::Direct(src)) = gather::gather(&[pools.as_slice()], amount, taxes) {
        return Ok(Ok(vec![pool(src)]));
    }
    // Решение 302: запасной источник — только при заданной presell.fund_fallback.
    let (held, held_err) = match dry_fund_holder::fallback(&q.ctx)? {
        Some(_) => match dry_fund_holder::usable(q, token, &legs, &at).await {
            Ok(u) => (Some(u), None),
            Err(e) => (None, Some(e.message)),
        },
        None => (None, None),
    };
    let holders: Vec<(Address, U256)> = held.as_ref().map_or(Vec::new(), |u| {
        u.ok.iter().map(|(a, b, _)| (*a, *b)).collect()
    });
    let label = |a: Address| {
        held.as_ref()
            .and_then(|u| u.ok.iter().find(|(h, _, _)| *h == a))
            .map_or_else(|| "пула".to_string(), |(_, _, l)| l.clone())
    };
    let reach = match gather::gather(&[pools.as_slice(), holders.as_slice()], amount, taxes) {
        Ok(Pick::Direct(src)) => {
            return Ok(Ok(vec![Fund {
                what: label(src),
                ..pool(src)
            }]));
        }
        Ok(Pick::Relay { hub, feeds }) => {
            let mut fs: Vec<Fund> = feeds
                .into_iter()
                .map(|(src, amount)| Fund {
                    src,
                    to: Some(hub),
                    amount,
                    what: label(src),
                })
                .collect();
            fs.push(Fund {
                what: label(hub),
                ..pool(hub)
            });
            return Ok(Ok(fs));
        }
        Err(reach) => reach,
    };
    let count = pools.len() + holders.len();
    let fundable = have
        .saturating_add(gather::net(reach, taxes).unwrap_or(U256::ZERO))
        .saturating_sub(margin(count));
    let mut why = format!(
        "у кошелька {have} единиц входа, нужно {need}; пула-источника (V2/V3 вне маршрута) с балансом ≥ {amount} нет — проверьте на сумме не больше баланса"
    );
    let mut no: Vec<String> = Vec::new();
    if let Some(u) = &held {
        no.extend(u.refused.iter().cloned());
        no.extend(
            u.ok.iter()
                .map(|(a, b, _)| format!("у держателя {a:#x} {b} единиц, нужно {amount}")),
        );
    }
    no.extend(held_err.map(|e| format!("держатели не прочитаны: {e}")));
    if !no.is_empty() {
        why.push_str(&format!(
            "; запасной источник ({} = {}) не взят: {}",
            dry_fund_holder::SETTING,
            dry_fund_holder::SINGLETON,
            no.join("; ")
        ));
    }
    // Решение 380: и все источники вместе меньше суммы — сколько можно проверить.
    why.push_str(&format!(
        "; все источники вместе дадут не больше {reach} единиц из {amount} — проверяемо не больше {fundable} единиц \
         входа (с запасом на округления; решение 380)"
    ));
    Ok(Err(Short { why, fundable }))
}

/// Пулы-источники токена для перевода в симуляции (решение 380: все с ненулевым балансом; выбор и набор —
/// exec/dry_fund_gather.rs). Принимает: котировку (реестр), токен, пулы маршрута (исключаются), блок чтения (тег:
/// блок симуляции). Возвращает: (адрес пула, баланс токена). Ошибки: узел (balanceOf).
pub async fn sources(
    q: &Quoting,
    token: Address,
    route: &[PoolRef],
    block: &str,
) -> GatewayResult<Vec<(Address, U256)>> {
    let pools: Vec<Address> = q
        .reg
        .all()
        .into_iter()
        .filter(|e| e.valid && matches!(e.pool.kind, PoolKind::V2 | PoolKind::V3))
        .filter(|e| e.pool.token0 == token || e.pool.token1 == token)
        .filter(|e| !route.contains(&e.pool.reference))
        .filter_map(|e| match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            PoolRef::Singleton { .. } | PoolRef::Member { .. } | PoolRef::Multi { .. } => None,
        })
        .collect();
    if pools.is_empty() {
        return Ok(Vec::new());
    }
    let calls: Vec<_> = pools
        .iter()
        .map(|a| IERC20::balanceOfCall { owner: *a })
        .collect();
    let balances = call::view_many(&q.ctx, token, &calls, block).await?;
    Ok(pools
        .into_iter()
        .zip(balances)
        .filter(|(_, b)| !b.is_zero())
        .collect())
}

/// Сумма перевода с учётом налога токена: недостача / (1 − налог), с округлением вверх. Принимает: недостачу, налоги
/// токена (None — без налога: USDT, промежуточная). Возвращает: сумму; None — налог ≥ 100 % или не считается.
/// Ошибок нет.
pub fn gross(deficit: U256, taxes: Option<&TokenTaxes>) -> Option<U256> {
    let pct = taxes.map_or(Decimal::ZERO, |t| t.buy_pct.max(t.transfer_pct));
    if pct.is_zero() {
        return Some(deficit);
    }
    let keep = Decimal::ONE - pct / Decimal::ONE_HUNDRED;
    if keep <= Decimal::ZERO {
        return None;
    }
    // Вычисление в целых единицах токена через Decimal (decimals 0): деление с округлением вверх.
    let d = num::to_decimal(deficit, 0)?;
    num::to_units((d / keep).ceil(), 0)
}
