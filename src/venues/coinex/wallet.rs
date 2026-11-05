//! Файл: venues/coinex/wallet.rs
//! Что делает: справочник монет и сетей CoinEx с кэшем в памяти не старше venue.networks_fresh_ms (решение 118): весь
//! список — GET /v2/assets/all-deposit-withdraw-config + адреса контрактов GET /v2/assets/info (оба ответа большие —
//! разбор вне рабочих потоков, за `Arc` без копий, решение 131); одна монета — те же запросы с ?ccy= (решение 113:
//! свежий статус одной монеты для предпроверки); стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs (Req, heavy), networks_parse.rs, contracts.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [coinex] — имя сети BSC в поле chain справочника ввода/вывода; venue.bsc_contract_chain
//! [coinex] — имя той же сети в chain_info.chain_name справочника /assets/info (в документации обоих имён нет — по живым
//! ответам, значений по умолчанию нет); venue.deposit_min_absent_ok, venue.networks_fresh_ms (необязательная) [coinex].
//!
//! Официально: Get Deposit and Withdrawal Configuration / List All… (группа «Query spot accounts (10r/1s)», Rate Limit)
//! https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-all-deposit-withdrawal-config; Get Coin/Token
//! Information https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-assets-info. Нужна ли подпись, на
//! страницах не сказано: эндпоинты раздела Assets в группе лимитов пользователя — запросы подписываются (проверить на
//! живом ответе, урок 2). Размер полного справочника не указан (урок 9, 28).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use alloy_primitives::Address;
use reqwest::Method;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::{Parsed, parse};
use super::rest::{self, Params, Req};

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал — пишем только при изменении.
    told: Arc<RwLock<String>>,
}

impl Networks {
    /// Справочник из кэша, если он моложе fresh (указатель, без копии). Ошибок нет.
    fn cached(&self, fresh: Duration) -> Option<Arc<Vec<CoinNetworkInfo>>> {
        let c = self.cache.read().unwrap_or_else(|p| p.into_inner());
        c.as_ref()
            .filter(|(at, _)| at.elapsed() < fresh)
            .map(|(_, l)| l.clone())
    }
}

/// Все монеты CoinEx и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Сети одной монеты — запросами с ?ccy= (решение 113). Ошибки: как coin_networks; монеты нет — пусто.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let (bsc, chain, min_ok) = names(ctx)?;
    let q = || vec![("ccy", ticker.to_string())];
    let info = get(ctx, "/v2/assets/info", q(), false).await?;
    let contracts = super::contracts::parse(&info, &chain)?;
    let data = get(ctx, "/v2/assets/deposit-withdraw-config", q(), false).await?;
    if data.is_null() {
        return Ok(vec![]);
    }
    let p = parse(&data, &bsc, &contracts.by_ticker, &ctx.venue(), min_ok)?;
    note(ctx, None, &p, &contracts.bad).await;
    Ok(p.list
        .into_iter()
        .filter(|i| i.asset.ticker.eq_ignore_ascii_case(ticker))
        .collect())
}

/// Имена сети BSC в двух справочниках и «минимум депозита неизвестен — 0». Ошибка NotConfigured.
fn names(ctx: &Ctx) -> GatewayResult<(String, String, bool)> {
    Ok((
        ctx.text("venue.bsc_network")?,
        ctx.text("venue.bsc_contract_chain")?,
        ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false),
    ))
}

/// Подписанный GET справочника (heavy — разбор вне рабочих потоков). Возвращает: data. Ошибки: как rest::send.
async fn get(
    ctx: &Ctx,
    path: &str,
    query: Params,
    heavy: bool,
) -> GatewayResult<serde_json::Value> {
    let r = Req {
        method: Method::GET,
        path,
        query,
        body: None,
        op: Op::Other,
        signed: true,
        heavy,
    };
    Ok(rest::data(rest::send(ctx, r).await?))
}

/// Справочник за `Arc`: из кэша — указатель; запрос — разбор вне рабочих потоков. Ошибки: как coin_networks.
async fn shared(ctx: &Ctx, nets: &Networks) -> GatewayResult<Arc<Vec<CoinNetworkInfo>>> {
    let fresh = ctx
        .int("venue.networks_fresh_ms")
        .ok()
        .map(Duration::from_millis);
    if let Some(list) = fresh.and_then(|f| nets.cached(f)) {
        return Ok(list);
    }
    let (bsc, chain, min_ok) = names(ctx)?;
    let info = get(ctx, "/v2/assets/info", vec![], true).await?;
    let data = get(ctx, "/v2/assets/all-deposit-withdraw-config", vec![], true).await?;
    let (c, venue) = (ctx.clone(), ctx.venue());
    let (p, bad) = tokio::task::spawn_blocking(move || {
        let contracts = super::contracts::parse(&info, &chain)?;
        let by: HashMap<String, Address> = contracts.by_ticker;
        Ok::<_, GatewayError>((parse(&data, &bsc, &by, &venue, min_ok)?, contracts.bad))
    })
    .await
    .map_err(|e| {
        c.tag(GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("разбор справочника монет CoinEx прерван: {e}"),
        ))
    })??;
    note(ctx, Some(nets), &p, &bad).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Журнал: пропущенные сети и особые сети BSC (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed, bad_contracts: &[String]) {
    let mut lines = vec![];
    let mut add = |items: &[String], what: &str| {
        if !items.is_empty() {
            lines.push(format!("{what}: {}", items.join(", ")));
        }
    };
    add(&p.skipped, "пропущены сети с неразборчивыми полями");
    add(
        &p.no_contract,
        "сети BSC без адреса контракта в /assets/info (venue.bsc_contract_chain) — монета не сопоставляется",
    );
    add(bad_contracts, "в /assets/info у цепи BSC identity не адрес");
    add(
        &p.memo,
        "сети BSC с memo для депозита — депозит не поддерживается",
    );
    add(
        &p.deflation,
        "сети BSC с deflation_rate (смысл не описан — комиссия вывода неизвестна, вывода нет)",
    );
    add(
        &p.delay,
        "сети BSC с deposit_delay_minutes (смысл не описан — вопрос Тиграну)",
    );
    let text = lines.join(" | ");
    if let Some(n) = nets {
        let mut t = n.told.write().unwrap_or_else(|e| e.into_inner());
        if *t == text {
            return;
        }
        *t = text.clone();
    }
    if !text.is_empty() {
        let m = format!("CoinEx справочник монет: {text}");
        ctx.log(Level::Warn, "venue.networks", m).await;
    }
}

/// Стоимость вывода: withdrawal_fee сети (в монетах; сети с deflation_rate — комиссия неизвестна).
/// Принимает: монету, сеть (chain). Ошибки: монеты или сети нет, комиссия неизвестна, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let list = coin_networks_of(ctx, &asset.ticker).await?;
    let info = list.iter().find(|c| c.network == network).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("У {} нет сети {network} в справочнике CoinEx", asset.ticker),
        ))
    })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "CoinEx: комиссия вывода {} в сети {network} неизвестна (нет withdrawal_fee или ненулевой deflation_rate)",
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
