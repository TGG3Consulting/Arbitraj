//! Сравнение финальной котировки Hashflow с полными затратами исполнимого маршрута той же площадки.
//! Индикативные уровни прогреваются в фоне. Торговая подпись хранится в памяти до expiry, отдельно от ручного preview.
use super::super::{best_of, exec, gas_ticker, num, parts::Parts, send, wallet};
use super::{Prepared, market};
use crate::core::types::{
    Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side, TradeRequest, TradeResult,
};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    hashflow::Request,
};
use alloy_primitives::{Address, B256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::time::timeout;

#[derive(Default)]
struct State {
    source: Option<(String, u64)>,
    market: Option<(Instant, Option<Arc<market::Book>>)>,
    loading: bool,
    signed: HashMap<(Address, B256), Arc<Prepared>>,
}

/// Состояние одной DEX. Привязанные кошельки разделяют уровни, подпись дополнительно индексируется по отправителю.
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<Mutex<State>>);

impl Book {
    fn depth(
        &self,
        p: &Parts,
        source: &str,
        chain: u64,
        max_age: Duration,
    ) -> Option<Arc<market::Book>> {
        let key = (source.to_owned(), chain);
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.source.as_ref() != Some(&key) {
            *state = State {
                source: Some(key.clone()),
                ..Default::default()
            };
        }
        if let Some((stamp, book)) = &state.market
            && stamp.elapsed() < max_age
        {
            return book.clone();
        }
        if !state.loading {
            state.loading = true;
            let ctx = p.ctx.clone();
            let book = self.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                let data = market::book(&ctx, &key.0, chain, None)
                    .await
                    .ok()
                    .map(Arc::new);
                let mut state = book.0.lock().unwrap_or_else(|e| e.into_inner());
                if state.source.as_ref() == Some(&key) {
                    state.loading = false;
                    state.market = Some((started, data));
                }
            });
        }
        None
    }
    fn save(&self, quote: Prepared) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state
            .signed
            .retain(|_, q| q.preview.expires > Utc::now().timestamp().max(0) as u64);
        state.signed.insert(
            (quote.request.trader, quote.firm.quote_data.txid),
            Arc::new(quote),
        );
    }
    pub(super) fn take(&self, wallet: Address, id: B256) -> Option<Arc<Prepared>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .signed
            .remove(&(wallet, id))
    }
}

/// Остаток срока относительно локального времени, дополнительно к часам симулированного блока.
pub(super) fn timely(p: &Parts, q: &Prepared) -> bool {
    let Ok(min) = p.ctx.int("hashflow.min_validity_ms") else {
        return false;
    };
    expiry_ok(q.preview.expires, min)
}

pub(super) fn expiry_ok(expiry: u64, min: u64) -> bool {
    let now = Utc::now().timestamp_millis().max(0) as u64;
    expiry
        .checked_mul(1000)
        .is_some_and(|end| end.saturating_sub(now) >= min)
}

/// Полный курс в USDT с реальными затратами вне цены. Отсутствующий курс исключает сравнение.
async fn total(p: &Parts, quote: &Quote) -> Option<Decimal> {
    let fee = best_of::extra::usdt(&p.quoting(), quote).await?;
    let gross = quote.avg_price.checked_mul(quote.qty)?;
    match quote.side {
        Side::Buy => gross.checked_add(fee),
        Side::Sell => gross.checked_sub(fee),
    }
}

fn better(side: Side, candidate: Decimal, base: Decimal) -> bool {
    match side {
        Side::Buy => candidate < base,
        Side::Sell => candidate > base,
    }
}

/// Внешний источник запрашивается только для финального объёма. Предварительные пробы не задерживают торги.
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: Quote,
) -> Quote {
    if !crate::gateway::quote_stage::is_final()
        || p.ctx.account.is_none()
        || !p.ctx.flag("hashflow.trading").unwrap_or(false)
        || baseline.fillable_qty != qty
        || baseline.qty != qty
        || baseline.route.as_ref().is_some_and(|r| !r.executable)
    {
        return baseline;
    }
    let Ok(ms) = p.ctx.int("hashflow.quote_budget_ms") else {
        return baseline;
    };
    match timeout(
        Duration::from_millis(ms),
        candidate(p, asset, side, qty, &baseline),
    )
    .await
    {
        Ok(Some(quote)) => quote,
        _ => baseline,
    }
}

async fn candidate(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: &Quote,
) -> Option<Quote> {
    let ctx = &p.ctx;
    let ticker = ctx.text("venue.quote_asset").ok()?;
    if !matches!(ticker.as_str(), "USDT" | "USDC") || exec::native::is_native(ctx, asset) {
        return None;
    }
    // RFQT ниже использует обычные ERC-20 Transfer и одно сообщение. Канал с отдельной транзакцией чаевых требует своей оценки.
    if send::tip(&send::select(ctx, &p.stats).ok()?).is_some() {
        return None;
    }
    let coin = asset.contract?;
    let quote = p.tokens.usdt(ctx).ok()?;
    let chain = super::super::node::chain_id::chain_id(ctx).ok()?;
    let source = ctx.text("hashflow.source").ok()?;
    if source.trim().is_empty() || source == "api" {
        return None;
    }
    let age = Duration::from_millis(ctx.int("hashflow.levels_max_age_ms").ok()?);
    let depth = p.rfqs.depth(p, &source, chain, age)?;
    let who = wallet::signer::address(ctx).ok()?;
    let buy = side == Side::Buy;
    let (input, output) = if buy { (quote, coin) } else { (coin, quote) };
    let req = Request {
        trader: who,
        input,
        output,
        amount: None,
        amount_tokens: Some(qty.normalize().to_string()),
    };
    let levels = depth.at(&req, qty, buy).ok()?;
    let old = total(p, baseline).await?;
    if !better(side, levels.1, old) {
        return None;
    }
    let prepared = super::check(ctx, chain, &source, req, buy, Some(levels))
        .await
        .ok()?;
    if !timely(p, &prepared) {
        return None;
    }
    let mut quoting = p.quoting();
    quoting.gas = quoting.gas.for_quote(ctx, &p.heads).await.ok()?;
    let bill = exec::cycle_gas::priced(
        &quoting,
        who,
        (
            prepared.firm.target_contract,
            prepared.firm.calldata.clone(),
        ),
    )
    .await
    .ok()?;
    let fees = exec::cycle_gas::simulated(&quoting, bill, prepared.gas, 0, 0).ok()?;
    let gas = num::to_decimal(fees.wei(), 18)?;
    let usdt = if buy {
        num::to_decimal(prepared.input, prepared.di)?
    } else {
        num::to_decimal(prepared.output, prepared.dout)?
    };
    let mut result = baseline.clone();
    result.avg_price = usdt.checked_div(qty)?;
    result.costs = vec![Cost::new(
        CostKind::Gas,
        gas,
        gas_ticker::read(ctx).ok()?,
        false,
    )];
    result.route = Some(DexRoute {
        mode: RouteMode::Rfq(prepared.firm.quote_data.txid),
        legs: vec![],
        block: prepared.block,
        gas_units: prepared.gas,
        executable: true,
        exec_note: None,
    });
    result.signal_route = None;
    result.at = Utc::now();
    let net = total(p, &result).await?;
    if !better(side, net, old) || !timely(p, &prepared) {
        return None;
    }
    result.pick = Some(format!(
        "Hashflow {} {ticker}, исходный маршрут {} {ticker} с газом",
        net.normalize(),
        old.normalize()
    ));
    p.rfqs.save(prepared);
    Some(result)
}

pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    super::execute::trade(p, req, id).await
}

pub(super) fn no(p: &Parts, s: &str) -> crate::gateway::errors::GatewayError {
    p.ctx.err(ErrorCode::NoRoute, s)
}
