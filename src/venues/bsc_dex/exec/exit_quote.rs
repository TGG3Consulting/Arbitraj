//! Файл: venues/bsc_dex/exec/exit_quote.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — продажа ВЫХОДА из позиции (TradeRequest.exit_position, ставит ядро) при
//! расхождении локального расчёта с квотером больше dex.quoter_max_diff_pct — не отказ, а продажа по выходу квотера:
//! квотер исполняет настоящий обмен на текущем состоянии и откатывается (research/flow/bsc_v4.md, раздел 5) — это ответ
//! сети. Ожидаемый выход пути умножается на квотер / расчёт (по каждому расходящемуся хопу, нарастающим итогом), и уже от
//! него — проверка худшей цены (exec/order.rs check_price) и минимум «получить не меньше» (exec/order.rs, exec/chain.rs —
//! с обычным exec.dex_slippage_pct; свежий минимум графа — тем же множителем). Квотер отказал, пул пуст, расчёт неполный,
//! «сравнить нельзя» — отказ, как раньше (exec/precheck.rs). Покупки, вход в сделку и сухая сделка — без поправки: у
//! них exit = false; решение 261 — сверка у них односторонняя (против нас — стоп, в нашу сторону — ExitFix с
//! favour = true: только строка в хронологию, exec/quoter_favour.rs). Циклы DEX ↔ DEX квотером не сверяются.
//! Живой случай 05.11 (сделка 20261105131254836GAIB, mexc → bsc_dex, 2434.66 GAIB, pancakeswap-infinity-clmm): расчёт
//! 42559024903490846447 против квотера 42489232805231828113 (0.164%) — авто-продажа и кнопки человека получали отказ по
//! кругу, пока допуск не подняли руками; продажа прошла по 0.01745.
//! Двух- и многопуловый путь: хоп сверяется на своей локальной сумме входа; множитель следующего хопа — к уже уменьшенному
//! выходу (линейная поправка: меньший вход даёт цену не хуже — минимум не завышается).
//! Кто вызывает: exec/trade_direct.rs (prepare), exec/precheck_quoter.rs (hops_exit — исполнитель, хопы Maverick),
//! exec/quoter_favour.rs (split).
//! От чего зависит: exec/precheck.rs (quoter_hop), quote/path.rs, num.rs, ctx.rs (журнал).
//! Настройки: через quoter_hop — dex.quoter [вид пула], dex.quoter_max_diff_pct [bsc_dex].
//! Журнал: dex.exit_by_quoter (Warn) — «расчёт X против квотера Y (Z%) — продажа по квотеру»; та же строка уходит в
//! хронологию сделки через TradeResult.executor (exec/result.rs by).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::quote::path::{Hop, Path};
use super::precheck;

/// Расхождение хопа с квотером, по которому продажа выхода идёт по квотеру.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitFix {
    /// «вид пула монета» — как в тексте отказа сверки.
    pub pool: String,
    /// Локальный выход хопа за вычетом крючка (минимальные единицы).
    pub local: U256,
    /// Выход квотера на ту же сумму.
    pub remote: U256,
    /// Расхождение, %.
    pub pct: Decimal,
    /// Решение 261: расхождение в нашу сторону у сделки, которая не продажа выхода (квотер больше расчёта) — не
    /// поправка, а только строка в хронологию: ожидаемое и минимум остаются по нашему расчёту.
    pub favour: bool,
}

impl ExitFix {
    /// Множитель квотер / расчёт. None — расчёт 0 или числа не помещаются в Decimal. Ошибок нет.
    pub fn ratio(&self) -> Option<Decimal> {
        let (l, r) = (
            num::to_decimal(self.local, 0)?,
            num::to_decimal(self.remote, 0)?,
        );
        if l.is_zero() {
            return None;
        }
        r.checked_div(l)
    }

    /// Строка для хронологии и журнала. Ошибок нет.
    pub fn text(&self) -> String {
        if self.favour {
            return format!(
                "{}: расчёт {}, квотер {} (+{}%) — в нашу сторону, сделка идёт (решение 261)",
                self.pool,
                self.local,
                self.remote,
                self.pct.round_dp(4)
            );
        }
        format!(
            "{}: расчёт {} против квотера {} ({}%) — продажа по квотеру",
            self.pool,
            self.local,
            self.remote,
            self.pct.round_dp(4)
        )
    }
}

/// Продажа ли выхода: признак ядра и сторона «продажа» (покупку с признаком адаптер не исполняет по квотеру). Ошибок нет.
pub fn exit_of(req: &TradeRequest) -> bool {
    req.exit_position && req.side == Side::Sell
}

/// Сверка хопов пути с квотером (решение 32), отобранных pick. exit = false — ровно quoter_now: расхождение против
/// нас — отказ; в нашу сторону (решение 261) — поправка с favour = true (её отделяет quoter_favour::split).
/// Принимает: окружение, путь, продажа ли выхода, какие хопы сверять. Возвращает: расхождения (номер хопа, цифры).
/// Ошибки: как у precheck::quoter_now (NoRoute, QuoteMismatch, NotConfigured, узел).
pub async fn check(
    ctx: &Ctx,
    path: &Path,
    exit: bool,
    pick: impl Fn(&Hop) -> bool,
) -> GatewayResult<Vec<(usize, ExitFix)>> {
    let mut out = Vec::new();
    for (i, h) in path.hops.iter().enumerate().filter(|(_, h)| pick(h)) {
        let fix = precheck::quoter_hop(
            ctx,
            &h.entry,
            h.token_in,
            &h.swapped,
            h.usable.hook_pct,
            exit,
        )
        .await?;
        if let Some(f) = fix {
            out.push((i, f));
        }
    }
    Ok(out)
}

/// Ожидаемое по квотеру: выход каждого хопа — на произведение множителей хопов до него включительно (минимум
/// промежуточного шага, exec/chain.rs), итог пути (USDT продажи) — на произведение всех. Обмен в пуле (Hop.swapped) не
/// трогается — по нему уже сверено. Принимает: путь, сторону, расхождения. Возвращает: общий множитель.
/// Ошибки: BadResponse — множитель не считается или переполнение.
pub fn rescale(
    ctx: &Ctx,
    path: &mut Path,
    side: Side,
    fixes: &[(usize, ExitFix)],
) -> GatewayResult<Decimal> {
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "множитель квотер / расчёт не считается",
        )
    };
    let mut r = Decimal::ONE;
    for (i, h) in path.hops.iter_mut().enumerate() {
        for (_, f) in fixes.iter().filter(|(j, _)| *j == i) {
            r = r.checked_mul(f.ratio().ok_or_else(bad)?).ok_or_else(bad)?;
        }
        h.pool_out = h.pool_out.checked_mul(r).ok_or_else(bad)?;
    }
    let out = match side {
        Side::Sell => &mut path.usdt,
        Side::Buy => &mut path.coins,
    };
    *out = out.checked_mul(r).ok_or_else(bad)?;
    Ok(r)
}

/// Свежие минимумы графа (решение 84) — тем же множителем (они посчитаны от локального расчёта). Ошибок нет.
pub fn scale_mins(mins: &[Decimal], r: Decimal) -> Vec<Decimal> {
    mins.iter().map(|m| *m * r).collect()
}

/// Журнал dex.exit_by_quoter и строка для хронологии сделки. Принимает: окружение, запрос, расхождения.
/// Возвращает: строку (None — расхождений нет). Ошибок нет.
pub async fn report(ctx: &Ctx, req: &TradeRequest, fixes: &[(usize, ExitFix)]) -> Option<String> {
    if fixes.is_empty() {
        return None;
    }
    let line = fixes
        .iter()
        .map(|(_, f)| f.text())
        .collect::<Vec<_>>()
        .join("; ");
    let msg = format!(
        "{} {} {}: продажа выхода — {line} (решение 177)",
        req.deal_id, req.qty, req.asset.ticker
    );
    ctx.log(Level::Warn, "dex.exit_by_quoter", msg, None).await;
    Some(line)
}
