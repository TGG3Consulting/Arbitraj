//! Файл: venues/bsc_dex/check/taxes.rs
//! Что делает: проверка налогов одной монеты (решение 30): пробный обмен в каждом её пуле (V2 впереди: только они
//! поддерживают монеты с налогом); газ обмена, разрешений и налоги монеты В ЭТОМ пуле (решение 146: налог зависит от
//! пула) — по каждому пулу в PoolFacts; налоги монеты для ядра — наименьшие по пулам (check/pool_taxes.rs).
//! Источник встречного токена для пробы — другой пул V2/V3 реестра с наибольшим его балансом (проверяемый пул не
//! трогается). Пул с промежуточной монетой (решение 52): сумма пробы check.probe_usdt пересчитывается в неё
//! (quote/rate.rs) — так налог монеты меряется и на этом шаге.
//! Итог — CoinCheck: налоги известны, или «проверка не прошла» с причиной (чёрный список у ядра) — только если пробный
//! обмен действительно выполнен и откатился (покупка — в каждом пуле; или покупка где-то прошла, но продажа откатилась
//! в каждом пуле без помех — решение 146). Нет достоверного пула, нет источника пробы, сбой узла, не задана
//! настройка — «не проверено» (NotChecked): это не свойство монеты, чёрного списка нет (Outcome::Wait / Config).
//! Кто вызывает: check/cycle.rs.
//! От чего зависит: check/probe.rs, check/pool_taxes.rs, check/native_probe.rs (нативная BNB, решение 62), quote/rate.rs, check/state.rs, pools/registry.rs, tokens.rs, num.rs, wallet/signer.rs, node/call.rs.
//! Настройки: check.probe_usdt [bsc_dex] (сумма пробы); через probe.rs — адреса Universal Router, Permit2, квотера,
//! exec.executor_address (пул вида без своего пути — проба через исполнитель, check/probe_exec.rs).
//!
//! Налог, % = (ожидалось − пришло) / ожидалось · 100 — research/flow/bsc_dex.md, раздел 6 (модель налога у
//! Uniswap SDK: buyFeeBps/sellFeeBps — доля, удерживаемая при переводе).

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::{Asset, CheckState, CoinCheck, PoolRef, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{rate, select};
use super::super::tokens::Tokens;
use super::super::wallet::signer;
use super::pool_taxes;
use super::state::Checks;
use super::taxes_util::{Sources, quote_source};
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
    /// Не задана настройка (dex.quoter, ключ кошелька…) — «не проверено»; второе — имена этих настроек (GatewayError.setting):
    /// перепроверка при задании именно их (решение 75).
    Config(String, Vec<String>),
}

/// Проверяет монету и пишет газ пулов в checks. Принимает: тикер, адрес монеты, USDT, головы сети (свежее время блока
/// для срока транзакции пробы берётся прямо перед каждой симуляцией — probe.rs).
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
    heads: &Heads,
) -> (CoinCheck, Outcome) {
    let out = match run(ctx, reg, checks, tokens, sources, ticker, usdt, heads).await {
        Ok(o) => o,
        Err(e) if matches!(e.code, ErrorCode::NotConfigured | ErrorCode::SecretsLocked) => {
            Outcome::Config(e.message, e.setting.into_iter().collect())
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
        Outcome::Wait(_) | Outcome::Config(..) => (CheckState::NotChecked, None),
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
        wait: match &out {
            Outcome::Wait(w) => Some(w.clone()),
            Outcome::Config(w, _) => Some(format!("не задана настройка: {w}")),
            _ => None,
        },
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
    heads: &Heads,
) -> GatewayResult<Outcome> {
    let wallet = signer::address(ctx)?;
    let usdt_dec = tokens.decimals(ctx, usdt).await?;
    let probe = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec).ok_or_else(|| {
        ctx.not_set(
            "check.probe_usdt",
            "check.probe_usdt не переводится в единицы USDT",
        )
    })?;
    let (pools, mut env, shallow) =
        super::taxes_util::probe_pools(ctx, reg, tokens, ticker, usdt_dec)?;
    if pools.is_empty() {
        // Пробовать нечем: причины по каждому пулу — глубина не считается, мелкий (с цифрами), спит (решение 73).
        let mut why = env;
        if why.is_empty() {
            why.push(super::tax_deep::SHALLOW.into());
        }
        why.extend(shallow);
        return Ok(Outcome::Wait(why.join("; ")));
    }
    // Решение 146: налоги каждого пула с удачной пробой; zero — у какого-то пула делить не на что (выход 0);
    // misses — почему не измерен налог продажи (откатилась ли продажа, причина).
    let (mut measured, mut zero, mut misses) = (Vec::new(), false, Vec::new());
    let (mut failed, mut config) = (Vec::new(), Vec::new());
    for e in &pools {
        let exclude = match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            PoolRef::Member { contract, .. } => Some(contract),
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
        let deep = |x: &Entry| {
            let any = select::Need::Any;
            matches!(
                select::deep_side(ctx, reg, tokens, x, usdt_dec, any),
                Ok(None)
            )
        };
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
        match probe::run(ctx, e, source, wallet, amount, heads).await {
            Ok(Ok(m)) => {
                let dec = (tokens.decimals(ctx, e.coin), tokens.decimals(ctx, e.usdt));
                let pt = pool_taxes::percent(&m, dec.0.await?, dec.1.await?);
                checks.update_pool(&e.pool.reference, |f| {
                    f.gas_buy = Some(m.gas_buy);
                    f.gas_approve_buy = Some(m.gas_approve_buy);
                    f.gas_sell = m.gas_sell.map(|g| g.0);
                    f.gas_approve_sell = m.gas_sell.map(|g| g.1);
                    f.taxes = pt.clone();
                });
                checks.set_transfer_gas(ticker, m.gas_transfer);
                // Нативная монета BNB (решение 62): газ WRAP/UNWRAP и нативного перевода.
                native_probe::after(ctx, checks, e, (source, wallet), amount, heads, &m).await;
                match pt {
                    Some(t) => {
                        if t.sell_pct.is_none() {
                            misses.push(pool_taxes::miss(&m, e.family.title));
                        }
                        measured.push(t);
                    }
                    None => zero = true,
                }
            }
            // Пробный обмен в пуле откатился — прежние налоги пула больше не действуют (пул в котировку не идёт).
            Ok(Err(why)) => {
                checks.update_pool(&e.pool.reference, |f| f.taxes = None);
                failed.push(format!("{}: {why}", e.family.title))
            }
            Err(err) if err.code == ErrorCode::NotConfigured => {
                config.push((err.message, err.setting))
            }
            Err(err) => env.push(format!("{}: {}", e.family.title, err.message)),
        }
    }
    Ok(pool_taxes::outcome(
        &measured,
        zero,
        &misses,
        (failed, env, config),
    ))
}
