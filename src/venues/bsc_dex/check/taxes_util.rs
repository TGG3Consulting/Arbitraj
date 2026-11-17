//! Файл: venues/bsc_dex/check/taxes_util.rs
//! Что делает: части проверки налогов: пулы монеты для пробы (глубокие в оба направления или мелкие, где пробный
//! круг проходит, — решение 393; порядок V2 → V3 → V4), налог в
//! процентах по паре «отправлено / дошло» и выбор источника встречного токена для пробы. Налоги пула по замеру пробы и
//! налоги монеты по пулам — check/pool_taxes.rs (решение 146).
//! Кто вызывает: check/taxes.rs, check/pool_taxes.rs, check/service_tax_calls.rs.
//! От чего зависит: pools/registry.rs, node/call.rs, num.rs, quote/select.rs, quote/side_fit.rs.
//! Настройки: через quote/select.rs — dex.min_pool_liquidity_usd; через quote/side_fit.rs — check.probe_usdt,
//! dex.quoter_max_diff_pct.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6.
//! balanceOf — EIP-20: https://eips.ethereum.org/EIPS/eip-20#balanceof

use std::collections::HashMap;
use std::sync::Mutex;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, PoolRef};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{select, shallow_pick, side_fit};
use super::super::tokens::Tokens;

/// Налог, %: (отправлено − дошло) / отправлено · 100 (дошло больше — 0). Принимает: суммы в минимальных единицах,
/// знаки токена. Возвращает: None — отправлено 0 или не переводится. Ошибок нет.
pub fn pct(sent: U256, got: U256, decimals: u8) -> Option<Decimal> {
    let s = num::to_decimal(sent, decimals)?;
    let g = num::to_decimal(got.min(sent), decimals)?;
    if s.is_zero() {
        None
    } else {
        Some((s - g) / s * Decimal::ONE_HUNDRED)
    }
}

/// Балансы встречного токена у пулов реестра — на один проход проверки налогов: пачка balanceOf по всем пулам
/// читается один раз на токен за проход, а не на каждую монету и пул (так проход стоит в разы меньше CU узла).
/// Баланс мог измениться за проход — тогда перевод из источника в пробе откатится, и это «не выполнена», не вердикт.
#[derive(Default)]
pub struct Sources {
    /// Токен → пулы по убыванию баланса.
    by_token: Mutex<HashMap<Address, Vec<(Address, U256)>>>,
}

/// Источник встречного токена (USDT или промежуточной монеты) для пробы: пул V2/V3 реестра (не проверяемый)
/// с наибольшим балансом этого токена ≥ суммы пробы. Кандидаты — только достоверные пулы с этим встречным токеном,
/// прошедшие фильтр ликвидности (`deep`): у мелких и чужих пулов баланс не нужен, пачка balanceOf в разы меньше.
/// Ошибки: узел. None — такого пула нет.
pub async fn quote_source(
    ctx: &Ctx,
    reg: &Registry,
    sources: &Sources,
    usdt: Address,
    exclude: Option<Address>,
    need: U256,
    deep: impl Fn(&Entry) -> bool,
) -> GatewayResult<Option<Address>> {
    let known = sources
        .by_token
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&usdt)
        .cloned();
    let list = match known {
        Some(l) => l,
        None => {
            let pools: Vec<Address> = reg
                .refs()
                .into_iter()
                // Решение 295: пары многомонетного пула (PoolRef::Multi) источником USDT не берутся — как пары
                // общего контракта (Member): только пул со своим контрактом и ссылкой Contract.
                .filter_map(|r| match (&r, reg.get(&r)) {
                    (PoolRef::Contract(a), Some(e)) if e.valid && e.usdt == usdt && deep(&e) => {
                        Some(*a)
                    }
                    _ => None,
                })
                .collect();
            let calls: Vec<_> = pools
                .iter()
                .map(|a| IERC20::balanceOfCall { owner: *a })
                .collect();
            let balances = call::view_many(ctx, usdt, &calls, "latest").await?;
            let mut l: Vec<(Address, U256)> = pools.into_iter().zip(balances).collect();
            l.sort_by_key(|x| std::cmp::Reverse(x.1));
            sources
                .by_token
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(usdt, l.clone());
            l
        }
    };
    Ok(list
        .into_iter()
        .find(|(a, b)| Some(*a) != exclude && *b >= need)
        .map(|(a, _)| a))
}

/// Пул для пробы и сумма пробы в нём: None — пул глубокий в оба направления, сумма — полная check.probe_usdt (как до
/// решения 393); Some(x) — мелкий пул, где пробный круг проходит на сумме x встречного токена (quote/side_fit.rs).
pub type ProbePool = (Entry, Option<U256>);

/// Пулы монеты для пробы (решение 393, t178): достоверные и либо глубокие в ОБА направления (решение 73, quote/select.rs
/// deep_side — сумма пробы полная), либо мелкие, но пробный круг — покупка на сумму пробы и продажа остатка назад —
/// проходит по расчёту пула целиком с выходом не меньше наименьшего (quote/side_fit.rs round_trip; не проходит полная
/// сумма — наибольшая из ряда сумма/2, сумма/4, …). Налог — свойство токена в этом пуле, меряется отношением «квотер /
/// пришло» на одной сумме (кривая пула сокращается), поэтому порог глубины для торговли к пробе не относится.
/// Порядок: V2 впереди (решение 39), затем V3, затем V4. Возвращает: (пулы с суммой, причины ожидания по пулам, где
/// глубина не считается, причины по мелким и спящим пулам, где и круг не проходит — с цифрами глубины; они — в текст
/// ожидания, только когда пробовать нечем). Сумму мелкого пула не посчитать (не задана check.probe_usdt или
/// dex.quoter_max_diff_pct, нет USDT или курса) — пул как мелкий (прежнее поведение).
/// Ревью t178 (Б2): мелкий пул — только если он в пределе dex.shallow_pools_per_pair пары (quote/shallow_pick.rs); не
/// задана или 0 — мелкие пулы в пробу не идут (как до решения 393).
/// Ошибки: NotConfigured (не задана настройка глубины).
pub fn probe_pools(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    coin: &Address,
    usdt_dec: u8,
) -> GatewayResult<(Vec<ProbePool>, Vec<String>, Vec<String>)> {
    let (mut pools, mut env, mut shallow) = (Vec::new(), Vec::new(), Vec::new());
    // Решение 211: пулы монеты — по адресу контракта.
    for e in reg.by_coin(coin).into_iter().filter(|e| e.valid) {
        let label = super::super::pools::depth_book::label(&e.pool.reference);
        match select::deep_side(ctx, reg, tokens, &e, usdt_dec, select::Need::Both) {
            Ok(None) => pools.push((e, None)),
            // Ревью t178 (Б2): мелкий пул — только в пределе dex.shallow_pools_per_pair на пару (quote/shallow_pick.rs).
            Ok(Some(why)) if !shallow_pick::admitted(ctx, reg, tokens, usdt_dec, &e) => {
                shallow.push(format!(
                    "{} {} {label}: {why}; мелкий пул вне предела {} = {} (или ни одна его сторона не пропускает сумму пробы)",
                    e.family.title,
                    e.ticker,
                    shallow_pick::NAME,
                    shallow_pick::limit(ctx)
                ))
            }
            Ok(Some(why)) => match trip(ctx, reg, tokens, &e, usdt_dec) {
                Ok(t) => pools.push((e, Some(t.amount))),
                Err(no) => shallow.push(format!(
                    "{} {} {label}: {why}; {no}",
                    e.family.title, e.ticker
                )),
            },
            // Не задана настройка глубины — ошибка с её именем (по нему ждущие перепроверяются при её задании).
            Err(why) => match super::super::pools::depth_book::missing_setting(ctx) {
                Some(name) => return Err(ctx.not_set(&name, why)),
                None => env.push(format!("{} {} {label}: {why}", e.family.title, e.ticker)),
            },
        }
    }
    // V2 впереди: только они поддерживают монеты с налогом (решение 39), затем V3, затем V4.
    pools.sort_by_key(|(e, _)| match e.pool.kind {
        PoolKind::V2 => 0,
        PoolKind::V3 => 1,
        _ => 2,
    });
    Ok((pools, env, shallow))
}

/// Пробный круг мелкого пула (quote/side_fit.rs round_trip) на сумме check.probe_usdt во встречном токене. Спящий пул
/// (при ws слушается только добавление ликвидности) — по его последнему состоянию: проба идёт на узле, а откат на
/// крошечном выходе не вердикт (check/probe_tiny.rs). Возвращает: круг; Err — почему не проходит (текст). Ошибок иных нет.
fn trip(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    e: &Entry,
    usdt_dec: u8,
) -> Result<side_fit::Trip, String> {
    let usdt = tokens.usdt(ctx).map_err(|x| x.message)?;
    let cap = side_fit::cap(ctx, reg, usdt, usdt_dec, e).map_err(|x| x.message)?;
    let min = side_fit::min_units(ctx).map_err(|x| x.message)?;
    side_fit::round_trip(ctx, e, cap, min).ok_or_else(|| {
        format!(
            "пробный круг на check.probe_usdt и меньших суммах (до выхода {min} единиц) по расчёту пула не проходит"
        )
    })
}

/// «Не проверено: пулы монеты ещё загружаются» для экранов. Принимает: окружение, тикер, адрес, причину. Ошибок нет.
pub fn loading_check(
    p: &super::super::pools::listen::Pools,
    ticker: &str,
    addr: Address,
    why: String,
) -> crate::core::types::CoinCheck {
    crate::core::types::CoinCheck {
        venue: p.ctx.venue(),
        asset: crate::core::types::Asset {
            ticker: ticker.to_string(),
            bsc_contract: Some(addr),
        },
        taxes: crate::core::types::CheckState::NotChecked,
        source: super::taxes::SOURCE.into(),
        checked_at: None,
        wait: Some(why),
    }
}

/// Причина ожидания — ядру, если у монеты ещё нет итога (прошла / не прошла) и причина сменилась. Принимает: окружение,
/// прошлое состояние, проверку с NotChecked и причиной. Ошибок нет.
pub async fn note_wait(
    p: &super::super::pools::listen::Pools,
    prev: &Option<crate::core::types::CheckState<crate::core::types::TokenTaxes>>,
    c: crate::core::types::CoinCheck,
) {
    note_wait_doubt(p, prev, c, false).await
}

/// Держится ли прежний итог при «не проверено» (чистая логика): «не прошла» — всегда; «прошла» — только без вердикта по
/// пулу (ревью t178, Б1: `doubt` — Outcome::Doubt). Ошибок нет.
pub fn keeps(
    prev: &Option<crate::core::types::CheckState<crate::core::types::TokenTaxes>>,
    doubt: bool,
) -> bool {
    use crate::core::types::CheckState;
    match prev {
        Some(CheckState::Failed { .. }) => true,
        Some(CheckState::Known(_)) => !doubt,
        _ => false,
    }
}

/// Как note_wait, а при `doubt` (Outcome::Doubt) прежний итог «прошла» заменяется на «не проверено» (keeps). Ошибок нет.
pub async fn note_wait_doubt(
    p: &super::super::pools::listen::Pools,
    prev: &Option<crate::core::types::CheckState<crate::core::types::TokenTaxes>>,
    c: crate::core::types::CoinCheck,
    doubt: bool,
) {
    if keeps(prev, doubt) {
        return;
    }
    // Был итог «прошла», снимается — причина могла совпасть с прошлой «не проверено» (её не было: был итог).
    let replacing = matches!(prev, Some(crate::core::types::CheckState::Known(_)));
    let old = p.checks.coin_of(&c.asset).and_then(|x| x.wait);
    if old == c.wait && !replacing {
        return;
    }
    p.checks.set_coin(c.clone());
    let _ = p
        .out
        .send(crate::gateway::events::MarketEvent::CoinCheck(c))
        .await;
}
