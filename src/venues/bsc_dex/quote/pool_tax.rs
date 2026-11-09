//! Файл: venues/bsc_dex/quote/pool_tax.rs
//! Что делает: налог монеты в котировке — ИМЕННО того пула, через который она идёт (решение 146; живой случай FLOKI
//! 04.11: налог только при обмене через зарегистрированные пулы, разный по пулам). Допуск пула по налогу стороны
//! (usable: измерен пробой и не больше порога), доля keep налога на шаге монеты маршрута (coin_keep) и налог шага для
//! затрат (step_pct), правило решения 39 по пулу (v2_only). Налог монеты из проверки (CoinCheck, наименьший по пулам) — только запасной: если монета сделки
//! в пуле шага — встречный токен (USDT / промежуточная), а не монета пула, налог именно этого пула для неё не мерился.
//! Кто вызывает: quote/select.rs (usable), quote/via.rs, quote/graph_route.rs, quote/graph_split.rs,
//! quote/verify_decide.rs, exec/live_hops.rs (coin_keep), quote/steps.rs (step_pct), quote/via_pools.rs,
//! quote/smart_ways.rs, exec/live.rs, exec/live_hops.rs (v2_only), exec/dry_swap.rs (max_buy).
//! От чего зависит: check/state.rs (PoolFacts, PoolTaxes), quote/route_calc.rs (Leg, keep), core/types, settings.
//! Настройки: tax.max_buy_pct, tax.max_sell_pct (глобальные; те же пороги, что у ядра — core/blacklist.rs).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{PoolKind, Side, TokenTaxes};
use crate::settings::read::At;

use super::super::check::state::PoolFacts;
use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::route_calc::{Leg, keep};
use super::select::Usable;

/// Налог монеты пула на сторону в этом пуле (решение 146) для допуска пула (quote/select.rs). Налог не измерен пробой —
/// пул по этой стороне не используется; больше порога tax.max_buy_pct / tax.max_sell_pct (то же сравнение «больше»,
/// что у ядра, core/blacklist.rs) — тоже. Порог не задан — причина «не задана настройка» (как у ядра: монета на DEX не
/// торгуется). Принимает: окружение, факты пула, сторону. Возвращает: налог, % (не меньше 0); Err — причина.
pub fn usable(ctx: &Ctx, facts: &PoolFacts, side: Side) -> Result<Decimal, String> {
    let (what, limit_name) = match side {
        Side::Buy => ("покупки", "tax.max_buy_pct"),
        Side::Sell => ("продажи", "tax.max_sell_pct"),
    };
    let pct = facts
        .taxes
        .as_ref()
        .and_then(|t| t.side(side))
        .ok_or_else(|| format!("налог {what} монеты в этом пуле не измерен пробным обменом"))?;
    // Решение 247, п. В: продажа выхода (монета уже куплена) — порог входа не применяется, налог идёт в модель как
    // измерен; реальный выход проверяет проба перед отправкой (и пере-замер при откате по minOut).
    if side == Side::Sell && crate::gateway::exit_scope::is_exit() {
        return Ok(pct.max(Decimal::ZERO));
    }
    let limit = ctx
        .settings
        .decimal(limit_name, At::default())
        .map_err(|e| e.message)?;
    if pct > limit {
        return Err(format!(
            "налог {what} монеты в этом пуле {}% больше {limit_name} ({limit}%)",
            pct.round_dp(4).normalize()
        ));
    }
    Ok(pct.max(Decimal::ZERO))
}

/// Налог монеты сделки на шаге пула, %: монета сделки — монета пула (Entry.coin) — налог этого пула на стороне
/// (Usable.tax_pct); иначе (монета сделки — встречный токен пула) — налог монеты из проверки (CoinCheck).
/// Принимает: пул шага, его допуск, монету сделки, сторону, налоги монеты из проверки. Ошибок нет.
pub fn step_pct(e: &Entry, u: &Usable, coin: Address, side: Side, t: &TokenTaxes) -> Decimal {
    if e.coin == coin {
        u.tax_pct
    } else if side == Side::Buy {
        t.buy_pct
    } else {
        t.sell_pct
    }
}

/// Доля keep налога монеты сделки на её шаге маршрута (решение 146): покупка — шаг, где монета выходит (последний),
/// продажа — где входит (первый); налог — пула этого шага (step_pct). Принимает: ноги в порядке исполнения, сторону,
/// налоги монеты из проверки. Возвращает: 1 − налог / 100; нет ног — 1 (маршрута нет — скажет route_calc::eval).
/// Ошибок нет.
pub fn coin_keep(legs: &[Leg], side: Side, t: &TokenTaxes) -> Decimal {
    let leg = match side {
        Side::Buy => legs.last().map(|l| (l, l.token_out)),
        Side::Sell => legs.first().map(|l| (l, l.token_in)),
    };
    leg.map_or(Decimal::ONE, |(l, coin)| {
        keep(step_pct(&l.entry, &l.usable, coin, side, t))
    })
}

/// Правило решения 39 по налогу ПУЛА (решение 146, Тигран 04.11): пул, у которого налог монеты на стороне шага больше 0,
/// допустим только если это пул V2; пул другого вида с измеренным налогом 0 — годен. Принимает: пул, его допуск.
/// Возвращает: Err(причина) — пул не берётся. Ошибок иных нет.
pub fn v2_only(e: &Entry, u: &Usable) -> Result<(), String> {
    if u.tax_pct > Decimal::ZERO && e.pool.kind != PoolKind::V2 {
        return Err(format!(
            "налог монеты в этом пуле {}% — пул с налогом допустим только V2 (решение 39)",
            u.tax_pct.round_dp(4).normalize()
        ));
    }
    Ok(())
}

/// Наибольший налог покупки монеты среди её пулов с удачной пробой (решение 146) — для запаса пополнения входа сухой
/// сделки (exec/dry_fund.rs gross: перевод из пула-источника токен может считать покупкой, а налог зависит от пула).
/// Принимает: котировку (реестр, итоги проверок), адрес монеты (решение 211). Возвращает: налог, %; пулов с замером нет
/// — 0 (тогда работает налог монеты из проверки). Ошибок нет.
pub fn max_buy(q: &super::Quoting, coin: &alloy_primitives::Address) -> Decimal {
    q.reg
        .by_coin(coin)
        .iter()
        .filter_map(|e| q.checks.pool(&e.pool.reference).taxes.map(|t| t.buy_pct))
        .max()
        .unwrap_or(Decimal::ZERO)
}
