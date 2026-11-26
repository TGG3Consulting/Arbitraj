//! Сравнение финальной котировки Hashflow с полными затратами исполнимого маршрута той же площадки.
//! Индикативные уровни прогреваются в фоне. Торговая подпись хранится в памяти до expiry, отдельно от ручного preview.
use super::super::{best_of, exec, gas_ticker, num, parts::Parts, send};
use super::{Prepared, market};
use crate::core::types::{
    Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side, TradeRequest, TradeResult,
};
use super::model::Request;
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
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

    signed: HashMap<(Address, B256), Arc<Prepared>>,
}

/// Состояние одной DEX. Привязанные кошельки разделяют уровни, подпись дополнительно индексируется по отправителю.
#[derive(Clone, Default)]
pub(in crate::venues::bsc_dex) struct Book(Arc<Mutex<State>>, super::availability::Cache);

impl Book {
    pub(super) fn populated(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).market.as_ref().is_some_and(|(_, book)| book.is_some())
    }
    pub(super) fn depth(
        &self,
        source: &str,
        chain: u64,
        max_age: Duration,
    ) -> Option<Arc<market::Book>> {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (name, id) = s.source.as_ref()?;
        if name != source || *id != chain {
            return None;
        }
        let (start, book) = s.market.as_ref()?;
        (start.elapsed() < max_age).then(|| book.clone()).flatten()
    }
    pub(super) fn install(
        &self,
        source: &str,
        chain: u64,
        start: Instant,
        data: Option<Arc<market::Book>>,
    ) {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.source
            .as_ref()
            .is_none_or(|(k, id)| k != source || *id != chain)
        {
            *s = State::default();
        }
        s.source = Some((source.to_owned(), chain));
        s.market = Some((start, data));
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
pub(super) async fn total(p: &Parts, quote: &Quote) -> Option<Decimal> {
    let mut q = p.quoting();
    q.tokens = q.tokens.pinned();
    let fee = best_of::extra::usdt(&q, quote).await?;
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

/// Полный размер и подтверждённый кошелёк определяются в памяти. Заблокированная проверкой монета сохраняет отказ.
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !super::enabled(&p.ctx) || qty <= Decimal::ZERO {
        return baseline;
    }
    if let Err(e) = &baseline
        && e.code != ErrorCode::NoRoute
    {
        return baseline;
    }
    let Some(input) = indicative(p, asset, side, qty) else {
        return baseline;
    };
    let complete = baseline.as_ref().ok().filter(|q| {
        q.qty == qty && q.fillable_qty == qty && q.route.as_ref().is_none_or(|r| r.executable)
    });
    let old = if let Some(q) = complete {
        let Some(value) = total(p, q).await else {
            return baseline;
        };
        Some(value)
    } else {
        None
    };
    let Some(cost) = total(p, &input.quote).await else {
        return baseline;
    };
    if old.is_some_and(|other| !better(side, cost, other)) {
        return baseline;
    }
    if !crate::gateway::quote_stage::is_final() {
        return Ok(input.quote);
    }
    if p.ctx.account.is_none() {
        return baseline;
    }
    let Ok(ms) = p.ctx.int("hashflow.quote_budget_ms") else {
        return baseline;
    };
    match timeout(
        Duration::from_millis(ms),
        candidate(p, side, qty, input, old),
    )
    .await
    {
        Ok(Some(q)) => Ok(q),
        _ => baseline,
    }
}

struct Indicative {
    quote: Quote,
    req: Request,
    chain: u64,
    source: String,
    levels: (Vec<String>, Decimal),
    cap: Decimal,
}

fn pair(
    p: &Parts,
    asset: &Asset,
    side: Side,
) -> Option<(u64, String, Duration, Address, Address, Arc<market::Book>)> {
    if exec::native::is_native(&p.ctx, asset) {
        return None;
    }
    let quote = p.tokens.usdt(&p.ctx).ok()?;
    let coin = asset.contract?;
    let (input, output) = if side == Side::Buy {
        (quote, coin)
    } else {
        (coin, quote)
    };
    let chain = super::super::node::chain_id::chain_id(&p.ctx).ok()?;
    let source = p.ctx.text("hashflow.source").ok()?;
    let age = Duration::from_millis(p.ctx.int("hashflow.levels_max_age_ms").ok()?);
    let depth = p.rfqs.depth(&source, chain, age)?;
    Some((chain, source, age, input, output, depth))
}

pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    asset: &Asset,
    side: Side,
    max: Decimal,
) -> Vec<Decimal> {
    if !super::enabled(&p.ctx) {
        return Vec::new();
    }
    pair(p, asset, side)
        .map(|(_, _, _, i, o, d)| d.breaks(i, o, side == Side::Buy, max))
        .unwrap_or_default()
}

fn indicative(p: &Parts, asset: &Asset, side: Side, qty: Decimal) -> Option<Indicative> {
    let ctx = &p.ctx;
    if !matches!(
        ctx.text("venue.quote_asset").ok()?.as_str(),
        "USDT" | "USDC"
    ) {
        return None;
    }
    let (chain, source, age, input, output, book) = pair(p, asset, side)?;
    let quote = p.quoting();
    super::super::quote::direct::taxes(&quote, asset).ok()?;
    let head = p.heads.fresh(ctx).ok()?;
    p.lag.check(ctx).ok()?;
    if send::tip(&send::select(ctx, &p.stats).ok()?).is_some() {
        return None;
    }
    let cap = ctx.dec("hashflow.gas_budget_native").ok()?;
    if cap <= Decimal::ZERO {
        return None;
    }
    // Эта проверка не вызывает RPC: на холодном кэше запускается только фоновое чтение.
    let funds = p.rfqs.1.get(p, chain, input, age)?;
    if funds.native < num::to_units(cap, 18)? {
        return None;
    }
    let buy = side == Side::Buy;
    let req = Request {
        trader: funds.who,
        input,
        output,
        amount: None,
        amount_tokens: Some(qty.normalize().to_string()),
    };
    let levels = book.at(&req, qty, buy).ok()?;
    let spend = if buy { levels.1 } else { qty };
    if funds.spend < num::to_units(spend, funds.decimals)? {
        return None;
    }
    let result = Quote {
        venue: ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: qty,
        avg_price: levels.1.checked_div(qty)?,
        costs: vec![Cost::new(
            CostKind::Gas,
            cap,
            gas_ticker::read(ctx).ok()?,
            false,
        )],
        route: Some(DexRoute {
            mode: RouteMode::RfqIndicative,
            legs: vec![],
            block: head.number,
            gas_units: 0,
            executable: false,
            exec_note: Some("Индикативная цена Hashflow, требуется подписанная котировка".into()),
        }),
        signal_route: None,
        at: Utc::now(),
        pick: Some("Индикативные уровни Hashflow с полным резервом газа".into()),
    };
    Some(Indicative {
        quote: result,
        req,
        chain,
        source,
        levels,
        cap,
    })
}

async fn candidate(
    p: &Parts,
    side: Side,
    qty: Decimal,
    i: Indicative,
    old: Option<Decimal>,
) -> Option<Quote> {
    let ctx = &p.ctx;
    let who = i.req.trader;
    let mut prepared = super::check(
        ctx,
        i.chain,
        &i.source,
        i.req,
        side == Side::Buy,
        Some(i.levels),
    )
    .await
    .ok()?;
    if !timely(p, &prepared) {
        return None;
    }
    let mut q = p.quoting();
    q.gas = q.gas.for_quote(ctx, &p.heads).await.ok()?;
    let bill = exec::cycle_gas::priced(
        &q,
        who,
        (
            prepared.firm.target_contract,
            prepared.firm.calldata.clone(),
        ),
    )
    .await
    .ok()?;
    let fees = exec::cycle_gas::simulated(&q, bill, prepared.gas, 0, 0).ok()?;
    let gas = num::to_decimal(fees.wei(), 18)?;
    if gas > i.cap {
        return None;
    }
    let gross = if side == Side::Buy {
        num::to_decimal(prepared.input, prepared.di)?
    } else {
        num::to_decimal(prepared.output, prepared.dout)?
    };
    let mut result = i.quote;
    result.avg_price = gross.checked_div(qty)?;
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
    let net = total(p, &result).await?;
    if old.is_some_and(|other| !better(side, net, other)) || !timely(p, &prepared) {
        return None;
    }
    result.at = Utc::now();
    let ticker = ctx.text("venue.quote_asset").ok()?;
    result.pick = Some(match old {
        Some(other) => format!(
            "Hashflow {} {ticker}, исходный маршрут {} {ticker} с газом",
            net.normalize(),
            other.normalize()
        ),
        None => format!(
            "Hashflow {} {ticker} с газом, доступна полная сумма",
            net.normalize()
        ),
    });
    prepared.fallback_fee = Some((net - gross).abs());
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
