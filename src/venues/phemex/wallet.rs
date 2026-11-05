//! Файл: venues/phemex/wallet.rs
//! Что делает: справочник сетей монет Phemex в памяти. Сводного справочника всех монет с сетями у Phemex нет — только
//! по одной монете за запрос (два подписанных запроса: депозит и вывод), поэтому весь справочник собирает фоновый круг
//! (networks_crawl.rs) в темпе venue.networks_requests_per_min, а coin_networks отдаёт то, что уже собрано (без
//! запросов). Одна монета (предпроверка, решение 113) — свежими запросами сразу. Стоимость вывода — по сумме вывода.
//! Кто вызывает: mod.rs (функции контракта), networks_crawl.rs.
//! От чего зависит: rest.rs, networks_parse.rs, networks_crawl.rs, symbols.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [phemex] — имя сети BSC (chainName; в документации примеры только BTC, ETH, TRX, EOS —
//! по живому ответу, значения по умолчанию нет), venue.deposit_min_absent_ok [phemex].
//!
//! Официально (Deposit And Withdraw REST API, https://phemex-docs.github.io/): chainCfg и asset/info — шапка
//! networks_parse.rs; оба с параметром currency (у asset/info он помечен необязательным, но что вернёт запрос без
//! монеты, в документации нет — не используется, проверить на живом). Группа лимитов Others — 100 запросов в минуту на
//! пользователя (Rate limits): справочник по монете делит её с балансами, выводом и историей.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use reqwest::Method;
use rust_decimal::Decimal;
use tokio::task::JoinHandle;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::merge;
use super::symbols::Symbols;
use super::{num, rest};

/// Путь настроек депозита по монете.
pub const DEPOSIT_CFG: &str = "/phemex-deposit/wallets/api/chainCfg";
/// Путь настроек вывода по монете.
pub const WITHDRAW_INFO: &str = "/phemex-withdraw/wallets/api/asset/info";

/// Справочник сетей по монете и фоновый круг сбора. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<HashMap<String, Arc<Vec<CoinNetworkInfo>>>>>,
    crawl: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Networks {
    /// Кладёт сети монеты в память. Ошибок нет.
    pub fn put(&self, ticker: &str, list: Vec<CoinNetworkInfo>) {
        let mut c = self.cache.write().unwrap_or_else(|p| p.into_inner());
        c.insert(ticker.to_string(), Arc::new(list));
    }

    /// Сколько монет уже собрано. Ошибок нет.
    pub fn len(&self) -> usize {
        self.cache.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Пусто ли. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Запускает фоновый круг сбора, если он не идёт. Ошибок нет.
    fn ensure_crawl(&self, ctx: &Ctx, symbols: &Symbols) {
        let mut h = self.crawl.lock().unwrap_or_else(|p| p.into_inner());
        if h.as_ref().is_none_or(|h| h.is_finished()) {
            let run = super::networks_crawl::run(ctx.clone(), symbols.clone(), self.clone());
            *h = Some(tokio::spawn(run));
        }
    }
}

/// Что делает: все собранные монеты и их сети (без запросов); первый вызов запускает фоновый круг сбора.
/// Ошибки: StaleData — ещё ни одной монеты не собрано (причина — в журнале круга).
pub async fn coin_networks(
    ctx: &Ctx,
    symbols: &Symbols,
    nets: &Networks,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    nets.ensure_crawl(ctx, symbols);
    let c = nets.cache.read().unwrap_or_else(|p| p.into_inner());
    if c.is_empty() {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::StaleData,
            None,
            "Справочник сетей Phemex ещё собирается (по монете за запрос) — подробности в журнале venue.networks",
        )));
    }
    Ok(c.values().flat_map(|l| l.iter().cloned()).collect())
}

/// Что делает: сети одной монеты свежими запросами (решение 113) и в память. Принимает: тикер.
/// Возвращает: сети монеты. Ошибки: настройки, ключи, связь, ответ Phemex.
pub async fn coin_networks_of(
    ctx: &Ctx,
    nets: &Networks,
    ticker: &str,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let q = || vec![("currency", ticker.to_string())];
    let dep = rest::signed(ctx, Method::GET, DEPOSIT_CFG, q(), None, Op::Other).await?;
    let wd = rest::signed(ctx, Method::GET, WITHDRAW_INFO, q(), None, Op::Other).await?;
    let p = merge(ticker, &dep, &wd, &bsc, &ctx.venue(), min_ok)?;
    if !p.skipped.is_empty() {
        let m = format!("Phemex сети {ticker}: пропущены {}", p.skipped.join("; "));
        ctx.log(crate::storage::journal::Level::Warn, "venue.networks", m)
            .await;
    }
    nets.put(ticker, p.list.clone());
    Ok(p.list)
}

/// Стоимость вывода: withdrawFeeRv сети для этой суммы (asset/info с amount — «withdraw fee required for user input
/// withdraw amount»). Принимает: монету, сеть (chainName), сумму. Ошибки: сети нет, комиссия не сообщена, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    asset: &Asset,
    network: &str,
    amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let mut q = vec![("currency", asset.ticker.clone())];
    if amount > Decimal::ZERO {
        q.push(("amount", amount.normalize().to_string()));
    }
    let data = rest::signed(ctx, Method::GET, WITHDRAW_INFO, q, None, Op::Other).await?;
    let chain = num::arr(&data, "chainInfos")?
        .iter()
        .find(|c| num::text_opt(c, "chainName").as_deref() == Some(network))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!(
                    "У {} нет сети {network} в настройках вывода Phemex",
                    asset.ticker
                ),
            ))
        })?;
    let fee = num::dec_opt(chain, "withdrawFeeRv")?.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Phemex: комиссия вывода {} в сети {network} не сообщена (нет withdrawFeeRv)",
                asset.ticker
            ),
        ))
    })?;
    Ok(vec![Cost::new(
        CostKind::WithdrawFee,
        fee,
        asset.ticker.clone(),
        false,
    )])
}
