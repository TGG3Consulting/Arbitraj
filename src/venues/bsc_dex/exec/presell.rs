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
//! после − до, т. е. после реального налога токена на реальной сумме реальным кошельком, минус излишек пары, который
//! создало само пополнение (решение 252, вариант «б», exec/dry_surplus_cut.rs; строка «излишек пары от пополнения: …»
//! — в «монеты в симуляции» журнала core.precheck и в причину отказа).
//! Нативная BNB (решение 62): продаётся value транзакции, в симуляции не пополняется — у кошелька меньше суммы —
//! NotApplicable с причиной (у нативной монеты нет контракта, налога токена нет).
//! Решение 303 (t134): план продажи прямого и умного пути собран с разрешениями, которых не хватает СЕЙЧАС
//! (wallet/approve.rs lacking: сумма или истёкший срок Permit2) — строка журнала dex.presell_approve: перед продажей
//! уйдёт approve и обмен подождёт его блока. Заранее разрешения не отправляются (вопрос Тиграну, docs/TASKS.md 303).
//! Решение 380 (t166): пополнить на весь объём нечем (у кошелька до покупки монет нет, источников мало — живой бот
//! 17.11: 350 отказов AVA, FLOKI, NXT, YCT, MIRA) — не отказ, а PresellOutcome::Short с объёмом, который можно проверить
//! (доля объёма запроса: можно / нужно единиц входа); ядро пересчитывает сделку на него и зовёт симуляцию снова
//! (core/precheck_dex_cut.rs) — сделка идёт только на объёме, проверенном симуляцией целиком.
//! Решение 382 (выравнивание с 177): расхождение с квотером у продажи выхода — продажа собрана по квотеру, как настоящая
//! (exec/dry_checks.rs), строка — в DexPresell.by_quoter и в текст отказа.
//! Кто вызывает: venue.rs (контракт dex_presell).
//! От чего зависит: exec/{dry_build, dry_sim, dry_swap (io), trade (sides), native}.rs, quote/{direct, pool_tax}.rs,
//! wallet/{signer, balances}.rs, num.rs.
//! Настройки: через вызываемые — exec.dex_slippage_pct, exec.executor_address, dex.*, smart.*, verify.*.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{DexPresell, PresellOutcome, PresellShort, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::num;
use super::super::quote::{Quoting, direct, pool_tax};
use super::super::wallet::{balances, signer};
use super::dry_build::{self, Built};
use super::dry_sim::{self, Io};
use super::executor::dry::DryMode;
use super::{dry_fund_gather, dry_surplus_cut, dry_swap, native, trade};

/// Симуляция продажи. Принимает: котировку (вид на счёт сделки), запрос продажи как у исполнения (маршрут из плана).
/// Возвращает: PresellOutcome::Done(DexPresell); решение 380 — Short: пополнить кошелёк на весь объём нечем, с
/// объёмом, который можно проверить (exec/dry_fund_gather.rs qty_share). Ошибки: NotConfigured (кошелёк не выбран, настройки), NoRoute (нет маршрута, сборка,
/// откат симуляции — с минимумами и ожидаемым), QuoteMismatch (цена хуже худшей), StaleData, BadResponse (выход не
/// прочитан), NotApplicable (нативная BNB без суммы на кошельке), узел.
pub async fn run(q: &Quoting, req: TradeRequest) -> GatewayResult<PresellOutcome> {
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
    // Решение 303: продаже сейчас не хватает разрешений (сумма или срок Permit2 истёк) — видно до покупки.
    if let Built::One(p) = &ready.built
        && !p.approvals.is_empty()
    {
        let msg = format!(
            "{} {}: продаже на DEX не хватает разрешений ({} шт.: сумма меньше продажи или срок Permit2 истёк) — \
             перед обменом уйдёт approve и обмен подождёт его блока; заранее не отправляется (решение 303)",
            req.deal_id,
            req.asset.ticker,
            p.approvals.len()
        );
        ctx.log(Level::Info, "dex.presell_approve", msg, None).await;
    }
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
    // Решение 252, вариант «б»: выход уже уменьшен на излишек пары от пополнения — строка в журнал и в причину отказа.
    let surplus = sim
        .surplus
        .map(|s| dry_surplus_cut::line(&s, decs.0, decs.1, "USDT"));
    let tail = || {
        format!(
            "кошелёк {wallet:#x}, форма {}, ожидалось {} USDT (живой пересчёт, модельный налог), минимум {} USDT \
             (по долям: {}), блок {}{}{}",
            ready.form,
            ready.usdt.round_dp(6),
            min_sum.round_dp(6),
            list(&min_legs),
            ready.at.number,
            surplus.as_ref().map_or(String::new(), |l| format!("; {l}")),
            ready
                .by_quoter
                .as_ref()
                .map_or(String::new(), |l| format!("; продажа по квотеру: {l}"))
        )
    };
    if let Some(why) = sim.reason {
        let why = format!("симуляция продажи до покупки не прошла: {why}; {}", tail());
        // Решение 380: причина — только нехватка источников пополнения: ядро пересчитает сделку на объём, который можно
        // проверить. Доля не считается (числа не переводятся) — как до решения: отказ.
        if let Some(fundable) = sim
            .fundable
            .and_then(|f| dry_fund_gather::qty_share(req.qty, f, io.need, decs.0))
        {
            return Ok(PresellOutcome::Short(PresellShort {
                need: req.qty,
                fundable,
                why,
            }));
        }
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
    Ok(PresellOutcome::Done(Box::new(DexPresell {
        wallet: format!("{wallet:#x}"),
        form: ready.form.clone(),
        block: ready.at.number,
        coins: ready.coins,
        out,
        expected: ready.usdt,
        min_legs,
        funding: match (sim.funding, surplus) {
            (Some(f), Some(l)) => Some(format!("{f}; {l}")),
            (f, l) => f.or(l),
        },
        gas_swap: sim.gas_swap,
        // Решение 382: продажа собрана по квотеру, как настоящая продажа выхода (решение 177).
        by_quoter: ready.by_quoter.clone(),
    })))
}

/// «1.23; 4.56» — минимумы долей для текста. Ошибок нет.
fn list(v: &[Decimal]) -> String {
    v.iter()
        .map(|x| x.round_dp(6).normalize().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}
