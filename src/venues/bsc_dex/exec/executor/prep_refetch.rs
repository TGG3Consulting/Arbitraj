//! Файл: venues/bsc_dex/exec/executor/prep_refetch.rs
//! Что делает: решение 382 — доли сделки через исполнитель (exec/executor/trade.rs prepare_once) с перечитыванием
//! пулов при расхождении расчёта по памяти с квотером против нас (хопы Maverick, exec/precheck_quoter.rs hops_exit):
//! пулы маршрута перечитываются на голове H (exec/quoter_refetch.rs pinned), доли считаются заново по ним, квотер —
//! на том же H; разошлось и на одном блоке — отказ с пометкой блока.
//! Кто вызывает: exec/executor/trade.rs (trade — до отправки и после разрешений).
//! От чего зависит: exec/executor/trade.rs (prepare_once), exec/quoter_refetch.rs, quote/via.rs.
//! Настройки: через вызываемые — см. exec/executor/trade.rs.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, TradeRequest};
use crate::gateway::errors::GatewayResult;

use super::super::super::quote::via::Via;
use super::super::chain::Step;
use super::super::quoter_refetch;
use super::trade::prepare_once;

/// Итог подготовки долей: шаги долей, строка «— продажа по квотеру», (ожидаемый выход, модельный налог) по долям,
/// строка «в нашу сторону» (как у exec/executor/trade.rs prepare_once).
pub type Prepared = (
    Vec<Vec<Step>>,
    Option<String>,
    Vec<(Decimal, Decimal)>,
    Option<String>,
);

/// Подготовка долей с перечитыванием при расхождении (см. шапку). Принимает: как prepare_once. Возвращает: Prepared.
/// Ошибки: как prepare_once; отказ на одном блоке — с пометкой (quoter_refetch::on_block).
pub async fn prepare(
    v: &Via<'_>,
    req: &TradeRequest,
    route: &DexRoute,
    mins: &[Decimal],
    decs: (u8, u8),
) -> GatewayResult<Prepared> {
    let first = match Box::pin(prepare_once(v, req, route, mins, decs)).await {
        Err(e) if quoter_refetch::wants(v.q, &e) => e,
        done => return done,
    };
    let pools = quoter_refetch::route_pools(route);
    let q = Box::pin(quoter_refetch::pinned(v.q, &pools, first)).await?;
    let v2 = quoter_refetch::via_on(v, &q);
    Box::pin(prepare_once(&v2, req, route, mins, decs))
        .await
        .map_err(|e| quoter_refetch::on_block(&q, e))
}
