//! Файл: venues/bsc_dex/exec/presell.rs
//! Что делает: симуляция продажи DEX ДО покупки (решение 234; живой случай ASS 08.11, сделки 20261108113609086ASS и
//! 20261108175020857ASS: план с налогом продажи 3.166 %, каждая продажа через исполнитель откатывалась «minOut»
//! contracts/ArbExecutor.sol — монету купили на MEXC и вывели, продать не смогли). Продажа собирается ТЕМ ЖЕ путём, что
//! настоящая (exec/dry_build.rs в режиме AsTrade — те же функции и порядок, что exec/trade.rs: прямой, умный, бандл,
//! исполнитель при exec.executor_address), кошельком вида на счёт сделки (Ctx::bound — маппинг), на количество запроса
//! (ожидаемый приход после вывода и налога перевода), с тем же минимумом выхода (exec/leg_min.rs от req.worst_price),
//! и прогоняется одной eth_simulateV1 (exec/dry_sim.rs). Монет у кошелька ещё нет: тот же приём, что у сухой сделки и
//! пробы налогов, — первым вызовом той же симуляции перевод монет на кошелёк от пула-источника ВНЕ маршрута
//! (exec/dry_fund.rs; пул маршрута не трогается, его резервы те же, что увидит продажа). Выход = баланс USDT кошелька
//! после − до, т. е. после реального налога токена на реальной сумме реальным кошельком.
//! Нативная BNB (решение 62): продаётся value транзакции, в симуляции не пополняется — у кошелька меньше суммы —
//! NotApplicable с причиной (у нативной монеты нет контракта, налога токена нет).
//! Кто вызывает: venue.rs (контракт dex_presell).
//! От чего зависит: exec/{dry_build, dry_sim, dry_swap (io), trade (sides), native}.rs, quote/{direct, pool_tax}.rs,
//! wallet/{signer, balances}.rs, num.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, exec.executor_address, dex.*, smart.*, verify.*.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{DexPresell, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::quote::{Quoting, direct, pool_tax};
use super::super::wallet::{balances, signer};
use super::dry_build::{self, Built};
use super::dry_sim::{self, Io};
use super::executor::dry::DryMode;
use super::{dry_swap, native, trade};

/// Симуляция продажи. Принимает: котировку (вид на счёт сделки), запрос продажи как у исполнения (маршрут из плана).
/// Возвращает: DexPresell. Ошибки: NotConfigured (кошелёк не выбран, настройки), NoRoute (нет маршрута, сборка,
/// откат симуляции — с минимумами и ожидаемым), QuoteMismatch (цена хуже худшей), StaleData, BadResponse (выход не
/// прочитан), NotApplicable (нативная BNB без суммы на кошельке), узел.
pub async fn run(q: &Quoting, req: TradeRequest) -> GatewayResult<DexPresell> {
    let ctx = &q.ctx;
    // Решение 132: только кошельком счёта сделки — кошелёк «пробы» здесь не годится.
    ctx.bound()?;
    if req.side != Side::Sell {
        return Err(ctx.err(ErrorCode::Internal, "симуляция до покупки — только продажа"));
    }
    let wallet = signer::address(ctx)?;
    let route = req
        .route
        .clone()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "в плане продажи нет маршрута DEX"))?;
    let ready = dry_build::build(q, &req, &route, wallet, DryMode::AsTrade).await?;
    let (coin, usdt, decs) = trade::sides(q, &req).await?;
    let is_native = native::is_native(ctx, &req.asset);
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма симуляции не переводится в число",
        )
    };
    let dec = |v: U256| num::to_decimal(v, decs.1).ok_or_else(bad);
    let min_legs = ready
        .legs
        .iter()
        .filter_map(|l| l.last())
        .map(|s| dec(s.order.min_out))
        .collect::<GatewayResult<Vec<Decimal>>>()?;
    let min_sum: Decimal = min_legs.iter().copied().sum();
    // Нативная BNB уходит value транзакции — в симуляции не пополняется (как у сухой сделки).
    if let Built::One(p) = &ready.built
        && !p.value().is_zero()
    {
        let have = balances::bnb(ctx, wallet).await?;
        let need = num::to_decimal(p.value(), num::BNB_DECIMALS).ok_or_else(bad)?;
        if have < need {
            let why = format!(
                "продажа BNB: у кошелька {have} BNB, нужно {need} — нативная монета в симуляции не пополняется \
                 (налога токена у неё нет)"
            );
            return Err(ctx.err(ErrorCode::NotApplicable, why));
        }
    }
    // Пополнение входа — с наибольшим налогом покупки среди пулов монеты (как сухая сделка, решение 146).
    let taxes = direct::taxes(q, &req.asset).ok().map(|mut t| {
        t.buy_pct = t.buy_pct.max(pool_tax::max_buy(q, &coin));
        t
    });
    let io = Io {
        taxes_in: taxes.as_ref(),
        ..dry_swap::io(&ready, Side::Sell, is_native, (coin, usdt))
    };
    let sim = dry_sim::run(q, &ready, wallet, &io).await?;
    let tail = || {
        format!(
            "кошелёк {wallet:#x}, форма {}, ожидалось {} USDT (живой пересчёт, модельный налог), минимум {} USDT \
             (по долям: {}), блок {}",
            ready.form,
            ready.usdt.round_dp(6),
            min_sum.round_dp(6),
            list(&min_legs),
            ready.at.number
        )
    };
    if let Some(why) = sim.reason {
        let why = format!("симуляция продажи до покупки не прошла: {why}; {}", tail());
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    let got = sim
        .out
        .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "выход USDT симуляции не прочитан"))?;
    let out = dec(got)?;
    if out < min_sum {
        let why = format!(
            "симуляция продажи до покупки: выход {} USDT меньше минимума; {}",
            out.round_dp(6),
            tail()
        );
        return Err(ctx.err(ErrorCode::NoRoute, why));
    }
    Ok(DexPresell {
        wallet: format!("{wallet:#x}"),
        form: ready.form.clone(),
        block: ready.at.number,
        coins: ready.coins,
        out,
        expected: ready.usdt,
        min_legs,
        funding: sim.funding,
        gas_swap: sim.gas_swap,
    })
}

/// «1.23; 4.56» — минимумы долей для текста. Ошибок нет.
fn list(v: &[Decimal]) -> String {
    v.iter()
        .map(|x| x.round_dp(6).normalize().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}
