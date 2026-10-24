//! Файл: venues/bsc_dex/check/taxes.rs
//! Что делает: проверка налогов одной монеты (решение 30): пробный обмен в каждом её пуле (налоги — по первому
//! удачному; V2 впереди: только они поддерживают монеты с налогом), газ обмена и разрешений — по каждому пулу.
//! Источник встречного токена для пробы — другой пул V2/V3 реестра с наибольшим его балансом (проверяемый пул не
//! трогается). Пул с промежуточной монетой (решение 52): сумма пробы check.probe_usdt пересчитывается в неё
//! (quote/rate.rs) — так налог монеты меряется и на этом шаге.
//! Итог — CoinCheck: налоги известны, или «проверка не прошла» с причиной (чёрный список у ядра) — только если пробный
//! обмен действительно выполнен и откатился. Нет достоверного пула, нет источника пробы, сбой узла, не задана
//! настройка — «не проверено» (NotChecked): это не свойство монеты, чёрного списка нет (Outcome::Wait / Config).
//! Кто вызывает: check/cycle.rs.
//! От чего зависит: check/probe.rs, check/native_probe.rs (нативная BNB, решение 62), quote/rate.rs, check/state.rs, pools/registry.rs, tokens.rs, num.rs, wallet/signer.rs, node/call.rs.
//! Настройки: check.probe_usdt [bsc_dex] (сумма пробы); через probe.rs — адреса Universal Router, Permit2, квотера.
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6 (модель налога у
//! Uniswap SDK: buyFeeBps/sellFeeBps — доля, удерживаемая при переводе).

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::{Asset, CheckState, CoinCheck, PoolKind, PoolRef, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{rate, select};
use super::super::tokens::Tokens;
use super::super::wallet::signer;
use super::state::Checks;
use super::taxes_util::{Sources, percent, quote_source};
use super::{native_probe, probe};

/// Имя источника проверки в CoinCheck.source (решение 30: источники подключаемые).
pub const SOURCE: &str = "eth_simulateV1";

/// Итог проверки монеты.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Налоги измерены.
    Known(TokenTaxes),
    /// Пробный обмен выполнен и откатился (или выход 0) — проверка не прошла: чёрный список у ядра.
    Failed(String),
    /// Проверить пока нельзя: нет достоверного пула, нет источника пробы, узел не ответил — «не проверено»,
    /// повтор, как только у монеты появится достоверный пул (pools/progress.rs).
    Wait(String),
    /// Не задана настройка (dex.quoter, check.probe_usdt, ключ кошелька…) — «не проверено», ошибка настройки.
    Config(String),
}

/// Проверяет монету и пишет газ пулов в checks. Принимает: тикер, адрес монеты, USDT, время блока (секунды).
/// Возвращает: CoinCheck (Known / Failed; ждёт пула или настройки — NotChecked) и итог. Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub async fn check_coin(
    ctx: &Ctx,
    reg: &Registry,
    checks: &Checks,
    tokens: &Tokens,
    sources: &Sources,
    ticker: &str,
    coin: Address,
    usdt: Address,
    block_time: u64,
) -> (CoinCheck, Outcome) {
    let out = match run(ctx, reg, checks, tokens, sources, ticker, usdt, block_time).await {
        Ok(o) => o,
        Err(e) if matches!(e.code, ErrorCode::NotConfigured | ErrorCode::SecretsLocked) => {
            Outcome::Config(e.message)
        }
        Err(e) => Outcome::Wait(e.message),
    };
    let (taxes, checked_at) = match &out {
        Outcome::Known(t) => (CheckState::Known(t.clone()), Some(Utc::now())),
        Outcome::Failed(reason) => (
            CheckState::Failed {
                reason: reason.clone(),
            },
            Some(Utc::now()),
        ),
        Outcome::Wait(_) | Outcome::Config(_) => (CheckState::NotChecked, None),
    };
    let check = CoinCheck {
        venue: ctx.venue(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: Some(coin),
        },
        taxes,
        source: SOURCE.to_string(),
        checked_at,
    };
    (check, out)
}

/// Пробы по пулам монеты. Возвращает: итог (см. Outcome). Ошибки: настройки, ключ, узел — вызывающий делает из них
/// Config / Wait.
#[allow(clippy::too_many_arguments)]
async fn run(
    ctx: &Ctx,
    reg: &Registry,
    checks: &Checks,
    tokens: &Tokens,
    sources: &Sources,
    ticker: &str,
    usdt: Address,
    block_time: u64,
) -> GatewayResult<Outcome> {
    let wallet = signer::address(ctx)?;
    let usdt_dec = tokens.decimals(ctx, usdt).await?;
    let probe = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            "check.probe_usdt не переводится в единицы USDT",
        )
    })?;
    // Сначала фильтр ликвидности: пулы мельче dex.min_pool_liquidity_usd не пробуются (quote/select.rs).
    let (mut pools, mut env) = (Vec::new(), Vec::new());
    for e in reg.by_ticker(ticker).into_iter().filter(|e| e.valid) {
        match select::deep_enough(ctx, reg, tokens, &e, usdt_dec) {
            Ok(None) => pools.push(e),
            Ok(Some(_)) => {}
            Err(why) if why.starts_with("Не задана настройка") => {
                return Err(ctx.err(ErrorCode::NotConfigured, why));
            }
            Err(why) => env.push(format!("{} {}: {why}", e.family.title, e.ticker)),
        }
    }
    // V2 впереди: только они поддерживают монеты с налогом (решение 39), затем V3, затем V4.
    pools.sort_by_key(|e| match e.pool.kind {
        PoolKind::V2 => 0,
        PoolKind::V3 => 1,
        _ => 2,
    });
    if pools.is_empty() && !env.is_empty() {
        return Ok(Outcome::Wait(env.join("; ")));
    }
    if pools.is_empty() {
        return Ok(Outcome::Wait(
            "нет достоверного пула ликвиднее dex.min_pool_liquidity_usd для пробного обмена".into(),
        ));
    }
    let mut taxes = None;
    let (mut failed, mut config) = (Vec::new(), Vec::new());
    for e in &pools {
        let exclude = match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            PoolRef::Singleton { .. } => None,
        };
        // Пул с промежуточной монетой (решение 52): сумма пробы — в ней, по живому пулу «промежуточная — USDT».
        let amount = match rate::from_usdt(ctx, reg, usdt, e.usdt, probe) {
            Ok(a) => a,
            Err(err) => {
                env.push(format!("{}: {}", e.family.title, err.message));
                continue;
            }
        };
        let deep =
            |x: &Entry| matches!(select::deep_enough(ctx, reg, tokens, x, usdt_dec), Ok(None));
        let source = match quote_source(ctx, reg, sources, e.usdt, exclude, amount, deep).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                env.push(format!(
                    "{}: нет другого пула с {amount} единиц {} для пробы",
                    e.family.title, e.usdt
                ));
                continue;
            }
            Err(err) => {
                env.push(format!("{}: {}", e.family.title, err.message));
                continue;
            }
        };
        match probe::run(ctx, e, source, wallet, amount, block_time).await {
            Ok(Ok(m)) => {
                checks.update_pool(&e.pool.reference, |f| {
                    f.gas_buy = Some(m.gas_buy);
                    f.gas_approve_buy = Some(m.gas_approve_buy);
                    f.gas_sell = m.gas_sell.map(|g| g.0);
                    f.gas_approve_sell = m.gas_sell.map(|g| g.1);
                });
                checks.set_transfer_gas(ticker, m.gas_transfer);
                // Нативная монета BNB (решение 62): газ WRAP/UNWRAP и нативного перевода.
                native_probe::after(ctx, checks, e, (source, wallet), amount, block_time, &m).await;
                if taxes.is_none() {
                    taxes = Some(percent(&m, tokens.decimals(ctx, e.coin).await?));
                }
            }
            Ok(Err(why)) => failed.push(format!("{}: {why}", e.family.title)),
            Err(err) if err.code == ErrorCode::NotConfigured => config.push(err.message),
            Err(err) => env.push(format!("{}: {}", e.family.title, err.message)),
        }
    }
    Ok(match taxes {
        Some(Some(t)) => Outcome::Known(t),
        Some(None) => {
            Outcome::Failed("пробный обмен: расчётный выход 0 — налог не определить".into())
        }
        // Вердикт — только если каждый пул дал вердикт: помеха на любом пуле (узел, 429) — проверка не выполнена.
        None if !config.is_empty() => Outcome::Config(config.join("; ")),
        None if !env.is_empty() => Outcome::Wait(env.join("; ")),
        None => Outcome::Failed(failed.join("; ")),
    })
}
