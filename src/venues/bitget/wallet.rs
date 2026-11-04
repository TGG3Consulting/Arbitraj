//! Файл: venues/bitget/wallet.rs
//! Что делает: справочник монет и сетей Bitget с кэшем в памяти не старше venue.networks_fresh_ms (решение 118):
//! весь список — GET /api/v2/spot/public/coins (ответ большой — разбор вне рабочих потоков, за `Arc` без копий, решение
//! 131); одна монета — тот же запрос с ?coin= (решение 113: свежий статус одной монеты для предпроверки); стоимость вывода.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs (Req, heavy), networks_parse.rs, ctx.rs, core/types.
//! Настройки: venue.bsc_network [bitget] — имя сети BSC (поле chain; в документации пример только «BTC» — по живому
//! ответу справочника, значения по умолчанию нет), venue.deposit_min_absent_ok [bitget], venue.networks_fresh_ms
//! [bitget] (необязательная).
//!
//! Официально: Get Coin Info («3 times/1s (IP)», параметр coin — «If the field is left blank, all coin information will
//! be returned») https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-coin-info — поля в
//! шапке networks_parse.rs. Официальный Python SDK зовёт /api/v2/spot/market/coins — расходится с документацией
//! (/api/v2/spot/public/coins); берётся путь документации (урок 2, проверить живым запросом).

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use reqwest::Method;

use crate::core::types::{Asset, CoinNetworkInfo, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::networks_parse::{Parsed, parse};
use super::rest::{self, Req};

/// Путь справочника монет (Get Coin Info).
const PATH: &str = "/api/v2/spot/public/coins";

/// Справочник и когда он получен.
type Cached = (Instant, Arc<Vec<CoinNetworkInfo>>);

/// Справочник сетей в памяти. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Networks {
    cache: Arc<RwLock<Option<Cached>>>,
    /// Последнее записанное в журнал (пропущенные; tag; доля комиссии; перегрузка) — пишем только при изменении.
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

/// Все монеты Bitget и их сети. Ошибки: настройки, связь, формат.
pub async fn coin_networks(ctx: &Ctx, nets: &Networks) -> GatewayResult<Vec<CoinNetworkInfo>> {
    Ok(Arc::unwrap_or_clone(shared(ctx, nets).await?))
}

/// Сети одной монеты — запросом с ?coin= (решение 113). Ошибки: как coin_networks; монеты нет — пусто.
pub async fn coin_networks_of(ctx: &Ctx, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let data = rest::public(ctx, PATH, vec![("coin", ticker.to_string())]).await?;
    if data.is_null() {
        return Ok(vec![]);
    }
    let p = parse(&data, &bsc, &ctx.venue(), min_ok)?;
    note(ctx, None, &p).await;
    // Ответ на ?coin= — массив; на всякий случай берём только запрошенную монету.
    Ok(p.list
        .into_iter()
        .filter(|i| i.asset.ticker.eq_ignore_ascii_case(ticker))
        .collect())
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
    let bsc = ctx.text("venue.bsc_network")?;
    let min_ok = ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false);
    let r = Req {
        method: Method::GET,
        path: PATH,
        query: vec![],
        body: None,
        op: Op::Other,
        signed: false,
        heavy: true,
    };
    let data = rest::send(ctx, r).await?;
    let (c, venue) = (ctx.clone(), ctx.venue());
    let p = tokio::task::spawn_blocking(move || parse(&data, &bsc, &venue, min_ok))
        .await
        .map_err(|e| {
            c.tag(GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("разбор {PATH} прерван: {e}"),
            ))
        })??;
    note(ctx, Some(nets), &p).await;
    let list = Arc::new(p.list);
    *nets.cache.write().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Журнал: пропущенные сети, сети BSC с tag, с долей комиссии вывода, в перегрузке (весь список — только при изменении).
async fn note(ctx: &Ctx, nets: Option<&Networks>, p: &Parsed) {
    let mut lines = vec![];
    let mut add = |items: &[String], what: &str| {
        if !items.is_empty() {
            lines.push(format!("{what}: {}", items.join(", ")));
        }
    };
    add(&p.skipped, "пропущены сети с неразборчивыми полями");
    add(
        &p.need_tag,
        "сети BSC с tag (needTag) — депозит не поддерживается",
    );
    add(
        &p.extra_fee,
        "сети BSC с extraWithdrawFee (доля от суммы; как считать — в документации нет: комиссия неизвестна, вывода нет)",
    );
    add(
        &p.congested,
        "сети BSC в перегрузке (congestion = congested)",
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
        let m = format!("Bitget coins: {text}");
        ctx.log(Level::Warn, "venue.networks", m).await;
    }
}

/// Стоимость вывода: withdrawFee сети (в монетах; сети с extraWithdrawFee — комиссия неизвестна).
/// Принимает: монету, сеть (chain). Ошибки: монеты или сети нет, комиссия неизвестна, связь.
pub async fn withdraw_cost(ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<Vec<Cost>> {
    let list = coin_networks_of(ctx, &asset.ticker).await?;
    let info = list.iter().find(|c| c.network == network).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("У {} нет сети {network} в справочнике Bitget", asset.ticker),
        ))
    })?;
    let fee = info.withdraw_fee.ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::BadResponse,
            None,
            format!(
                "Bitget: комиссия вывода {} в сети {network} неизвестна (нет withdrawFee или ненулевой extraWithdrawFee)",
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
