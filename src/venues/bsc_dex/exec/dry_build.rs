//! Файл: venues/bsc_dex/exec/dry_build.rs
//! Что делает: сборка «сухой сделки DEX» (решение 121, C4) — тем же путём, что сделка, но без занятия очереди кошелька
//! (nonce) и без отправки: те же функции exec/* и quote/*, вызванные в том же порядке, что в exec/trade.rs,
//! exec/trade_smart.rs и exec/bundle_trade.rs. Графовый маршрут — предторговая верификация (quote/verify::run, как
//! verify_gate::gate, но без уведомления exec.verified / exec.verify_rejected — сухая сделка их не шлёт); форма —
//! quote/graph_route::graph_form. Прямой: живой путь (exec/live.rs, прямой режим), цена против худшей, шаги
//! (exec/chain.rs), сверка с квотером (exec/precheck.rs), план Universal Router своей DEX (exec/plan.rs, недостающие
//! разрешения — с узла). Умный / граф «один UR»: доли по живому состоянию, сверка с MixedQuoter, exec/universal.rs.
//! Бандл: доли, свежие минимумы ног из верификации, exec/bundle.rs (первый nonce 0 — в симуляции nonce не нужен).
//! Доли по живому состоянию и верификация графа — exec/dry_paths.rs. Тело одной сборки — exec/dry_build_once.rs, сверки с
//! квотером — exec/dry_checks.rs.
//! Через исполнитель ArbExecutor (решение 122, часть X) — exec/executor/dry.rs: режим «через исполнитель» (любой маршрут,
//! код подставлен в симуляцию) или графовый «бандл» при заданном exec.executor_address (как сделка).
//! Решение 382: расхождение расчёта по памяти с квотером против нас (любая форма со сверкой): пулы маршрута
//! перечитываются на голове, сборка заново по ним, квотер — на том же блоке (exec/quoter_refetch.rs).
//! Кто вызывает: exec/dry_swap.rs, exec/presell.rs.
//! Решение 122 F: в графовой форме хопы через пулы Maverick V2 сверяются с квотером (exec/precheck_quoter.rs).
//! От чего зависит: exec/dry_build_once.rs, exec/quoter_refetch.rs (через once — exec/{trade, live, chain, leg_min,
//! dry_checks, plan, universal, bundle, native}.rs, quote/{verify, graph_route, direct, via, steps}.rs).
//! Настройки: через вызываемые — exec.dex_slippage_pct, dex.*, smart.*, verify.*.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, TradeRequest};
use crate::gateway::errors::GatewayResult;

use super::super::quote::Quoting;
use super::super::{check::simulate::Base, pools::registry::Entry};
use super::bundle::Bundle;
use super::chain::Step;
use super::dry_build_once::once;
use super::executor::dry::{DryMode, ExecTx};
use super::plan::Plan;
use super::quoter_refetch;

/// Что собрано: одна транзакция Universal Router (прямой / умный), бандл или вызов исполнителя ArbExecutor (решение 122).
pub enum Built {
    One(Plan),
    Many(Bundle),
    Exec(ExecTx),
}

/// Собранная сухая сделка.
pub struct Ready {
    pub built: Built,
    /// Шаги каждой доли (минимумы, входы).
    pub legs: Vec<Vec<Step>>,
    /// Монет и USDT по живому пересчёту (вся сделка).
    pub coins: Decimal,
    pub usdt: Decimal,
    pub form: String,
    /// Решения 177, 382: продажа выхода прямой формы — по квотеру: строка расхождений (None — по расчёту).
    pub by_quoter: Option<String>,
    /// Блок, по времени которого считан срок; поверх него — симуляция (через исполнитель — со временем головы + 1 с).
    pub at: Base,
}

/// Собирает сделку. Принимает: котировку, запрос (как для сделки), маршрут, кошелёк. Возвращает: Ready.
/// Ошибки: как у сделки до отправки — NoRoute, QuoteMismatch, StaleData, CoinNotChecked, NotConfigured, узел.
/// Решение 382: расчёт по памяти разошёлся с квотером против нас: пулы маршрута перечитываются на голове H
/// (exec/quoter_refetch.rs) и сборка идёт заново по ним, квотер — на том же H (как сделка).
pub async fn build(
    q: &Quoting,
    req: &TradeRequest,
    route: &DexRoute,
    wallet: alloy_primitives::Address,
    mode: DryMode,
) -> GatewayResult<Ready> {
    let first = match Box::pin(once(q, req, route, wallet, mode)).await {
        Err(e) if quoter_refetch::wants(q, &e) => e,
        done => return done,
    };
    let pools = quoter_refetch::route_pools(route);
    let q2 = Box::pin(quoter_refetch::pinned(q, &pools, first)).await?;
    Box::pin(once(&q2, req, route, wallet, mode))
        .await
        .map_err(|e| quoter_refetch::on_block(&q2, e))
}

/// Пулы шагов всех долей по порядку. Ошибок нет.
pub fn entries(legs: &[Vec<Step>]) -> Vec<&Entry> {
    legs.iter().flatten().map(|s| &s.entry).collect()
}
