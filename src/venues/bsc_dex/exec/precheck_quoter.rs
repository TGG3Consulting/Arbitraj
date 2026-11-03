//! Файл: venues/bsc_dex/exec/precheck_quoter.rs
//! Что делает: сверка с квотером своей DEX каждого хопа маршрута, посчитанного портом чужой математики, прямо перед
//! сделкой через исполнитель (решение 73; решение 122 F — Maverick V2: расчёт — порт команды Maverick, Solidity-исходник
//! закрыт, поэтому каждый обмен через такой пул сверяется с MaverickV2Quoter.calculateSwap на своём узле на сумме
//! хопа). Расхождение больше dex.quoter_max_diff_pct, неполный расчёт или откат квотера — сделки нет (exec/precheck.rs
//! quoter_now); пул из котировок исключает цикл сверки (check/quoter_cycle.rs).
//! Кто вызывает: exec/executor/trade.rs (prepare — доли перед отправкой).
//! От чего зависит: exec/precheck.rs (quoter_now), quote/path.rs, pools/families.rs.
//! Настройки: через quoter_now — dex.quoter [DEX], dex.quoter_max_diff_pct [bsc_dex].

use rust_decimal::Decimal;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::pools::families::Proto;
use super::super::quote::path::Path;
use super::precheck;

/// Виды пулов, у которых расчёт — порт без официального исходника: их хоп сверяется квотером перед каждой сделкой.
/// Ошибок нет.
pub fn must_quote(p: Proto) -> bool {
    p == Proto::Maverick
}

/// Сверка хопов таких пулов во всех долях. Принимает: окружение, доли (путь, количество). Ошибки: как у quoter_now
/// (NoRoute, QuoteMismatch, NotConfigured, узел).
pub async fn hops(ctx: &Ctx, paths: &[(Path, Decimal)]) -> GatewayResult<()> {
    for h in paths.iter().flat_map(|(p, _)| p.hops.iter()) {
        if must_quote(h.entry.family.proto) {
            precheck::quoter_now(ctx, &h.entry, h.token_in, &h.swapped, h.usable.hook_pct).await?;
        }
    }
    Ok(())
}
