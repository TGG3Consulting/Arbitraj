//! Файл: venues/bsc_dex/exec/dry_swap.rs
//! Что делает: «сухая сделка DEX» (решение 121, C4) — приёмка своего сборщика свопа перед деньгами. Монета, сторона,
//! сумма USDT → количество монет подбирается котировкой под сумму (спот-цена пулов реестра → котировка → уточнение
//! второй котировкой) → маршрут той котировки (quote — тот же путь, что у живого экрана и ядра) → худшая цена как у
//! ядра (core/worst.rs: средняя × (1 ± exec.dex_slippage_pct)) → сборка КАК ДЛЯ СДЕЛКИ (exec/dry_build.rs) → одна
//! симуляция eth_simulateV1 от кошелька бота с разрешениями первыми вызовами (exec/dry_sim.rs). Ничего не подписывает
//! и не отправляет, очередь кошелька и nonce не трогает, уведомлений сделки не шлёт, пулы не исключает.
//! Итог — DexDrySwap: форма маршрута, пулы, выход по котировке / живому пересчёту / минимум / симуляции, газ, причина.
//! Подбор количества под сумму USDT — exec/dry_size.rs.
//! Режим «через исполнитель» (решение 122, часть X): маршрут — шагами ArbExecutor, код подставлен в симуляцию
//! (exec/executor/dry.rs) — проверка исполнителя на живой сети до развёртывания.
//! Кто вызывает: venue.rs (контракт dex_dry_swap).
//! От чего зависит: quote/mod.rs (Quoting::quote), quote/spot.rs, exec/{dry_build, dry_sim}.rs, wallet/{signer,
//! balances}.rs, num.rs.
//! Настройки: exec.dex_slippage_pct, venue.quote_asset [bsc_dex]; через вызываемые — как у котировки и сделки.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Asset, DexDrySwap, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::quote::{Quoting, direct, pool_tax};
use super::super::wallet::{balances, signer};
use super::dry_build::{self, Built, Ready};
use super::dry_sim::{self, Io};
use super::dry_size;
use super::executor::dry::DryMode;
use super::native;

/// Сухая сделка. Принимает: котировку, монету, сторону, сумму USDT, режим (как сделка / через исполнитель с
/// подставленным кодом — решение 122). Возвращает: итог (отказ — reason). Ошибок нет.
pub async fn run(
    q: &Quoting,
    asset: Asset,
    side: Side,
    usdt: Decimal,
    mode: DryMode,
) -> DexDrySwap {
    let mut r = DexDrySwap::new(q.ctx.venue(), asset, side, usdt);
    if mode == DryMode::Executor {
        r.notes.push(
            "режим «через исполнитель»: маршрут собран шагами ArbExecutor, код исполнителя и его владелец (кошелёк) \
             подставлены в симуляцию на будущий адрес развёртывания — в сети ничего не создаётся"
                .into(),
        );
    }
    if let Err(e) = fill(q, &mut r, mode).await {
        r.reason = Some(e.message);
    }
    r
}

/// Тело run: заполняет итог по шагам. Ошибки: любая на пути — становится причиной.
async fn fill(q: &Quoting, r: &mut DexDrySwap, mode: DryMode) -> GatewayResult<()> {
    let ctx = &q.ctx;
    if r.usdt <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::Internal, "сумма USDT должна быть больше 0"));
    }
    let wallet = signer::address(ctx)?;
    let quote = dry_size::sized(q, r).await?;
    let route = quote
        .route
        .clone()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "у котировки нет маршрута DEX"))?;
    r.pools = dry_size::pools(&route);
    r.out_symbol = Some(match r.side {
        Side::Buy => r.asset.ticker.clone(),
        Side::Sell => ctx.text("venue.quote_asset")?,
    });
    r.quote_out = Some(route.legs.iter().map(|l| l.expected_out).sum());
    let slip = ctx.dec("exec.dex_slippage_pct")? / Decimal::ONE_HUNDRED;
    let worst = match r.side {
        Side::Buy => quote.avg_price * (Decimal::ONE + slip),
        Side::Sell => quote.avg_price * (Decimal::ONE - slip),
    };
    let req = TradeRequest {
        venue: ctx.venue(),
        asset: r.asset.clone(),
        side: r.side,
        qty: quote.fillable_qty,
        worst_price: worst,
        deal_id: "dry-swap".into(),
        route: Some(route.clone()),
        // Решение 177: сухая сделка проверяет сборку как есть — без продажи по квотеру.
        exit_position: false,
        remeasure: None,
    };
    let ready = dry_build::build(q, &req, &route, wallet, mode).await?;
    r.form = Some(ready.form.clone());
    r.live_out = Some(match r.side {
        Side::Buy => ready.coins,
        Side::Sell => ready.usdt,
    });
    r.block = Some(ready.at.number);
    let (coin, usdt_t, decs) = super::trade::sides(q, &req).await?;
    let is_native = native::is_native(ctx, &r.asset);
    let out_dec = match (r.side, is_native) {
        (Side::Sell, _) => decs.1,
        (Side::Buy, true) => num::BNB_DECIMALS,
        (Side::Buy, false) => decs.0,
    };
    let dec = |v: U256, d: u8| num::to_decimal(v, d);
    let min_units: U256 = ready
        .legs
        .iter()
        .filter_map(|l| l.last())
        .fold(U256::ZERO, |a, s| a.saturating_add(s.order.min_out));
    r.min_out = dec(min_units, out_dec);
    let io = io(&ready, r.side, is_native, (coin, usdt_t));
    // Продажа BNB (решение 62): BNB уходит value транзакции — в симуляции он не пополняется.
    if let Built::One(p) = &ready.built
        && !p.value().is_zero()
    {
        let have = balances::bnb(ctx, wallet).await?;
        let need = dec(p.value(), num::BNB_DECIMALS).unwrap_or_default();
        if have < need {
            let why = format!(
                "продажа BNB: у кошелька {have} BNB, нужно {need} — BNB в симуляции не пополняется, проверьте на сумме не больше баланса"
            );
            return Err(ctx.err(ErrorCode::InsufficientBalance, why));
        }
    }
    // Решение 146: пополнение входа продажи — с НАИБОЛЬШИМ налогом покупки среди пулов монеты (пул-источник любой, налог
    // зависит от пула; в CoinCheck — наименьший), quote/pool_tax.rs max_buy.
    let taxes = direct::taxes(q, &r.asset).ok().map(|mut t| {
        t.buy_pct = t.buy_pct.max(pool_tax::max_buy(q, &coin));
        t
    });
    let io = Io {
        taxes_in: if r.side == Side::Sell {
            taxes.as_ref()
        } else {
            None
        },
        ..io
    };
    let sim = dry_sim::run(q, &ready, wallet, &io).await?;
    r.gas_swap = Some(sim.gas_swap);
    r.gas_approve = Some(sim.gas_approve);
    r.approvals = sim.approvals;
    r.funding = sim.funding;
    r.sim_out = sim.out.and_then(|v| dec(v, out_dec));
    if io.token_out.is_none() && sim.out.is_none() {
        r.notes
            .push("выход BNB не измерен: не задан dex.multicall3 (getEthBalance)".into());
    }
    r.reason = sim.reason;
    if r.reason.is_none() {
        match (sim.out, min_units) {
            (Some(got), min) if got < min => {
                r.reason = Some(format!(
                    "выход по симуляции {got} < минимума {min} (в единицах токена)"
                ));
            }
            _ => r.ok = true,
        }
    }
    Ok(())
}

/// Вход и выход сделки: токен входа (None — платит роутер из BNB транзакции), сумма входа кошелька (сумма первых
/// шагов долей, которые платит кошелёк), токен выхода (None — нативный BNB). Ошибок нет.
pub fn io(
    r: &Ready,
    side: Side,
    is_native: bool,
    (coin, usdt): (alloy_primitives::Address, alloy_primitives::Address),
) -> Io<'static> {
    let need = r
        .legs
        .iter()
        .filter_map(|l| l.first())
        .filter(|s| s.from_wallet)
        .fold(U256::ZERO, |a, s| a.saturating_add(s.order.amount_in));
    let token_in = match side {
        Side::Buy => Some(usdt),
        Side::Sell if is_native => None,
        Side::Sell => Some(coin),
    };
    let token_out = match side {
        Side::Sell => Some(usdt),
        Side::Buy if is_native => None,
        Side::Buy => Some(coin),
    };
    Io {
        token_in: token_in.filter(|_| !need.is_zero()),
        need,
        taxes_in: None,
        token_out,
    }
}
