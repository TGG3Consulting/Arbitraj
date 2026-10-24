//! Файл: venues/bsc_dex/pools/dex_rules.rs
//! Что делает: правила по строке таблицы сопоставления DEX (решения 38, 58, 59): исполняет ли её пулы умный роутер
//! (smart_ok), не изменился ли её вид на лету (still), список DEX для интерфейса (for_ui).
//! Кто вызывает: quote/select.rs, quote/smart_*.rs, exec/universal.rs, mod.rs (через families.rs).
//! От чего зависит: ctx.rs, families.rs (classify, all), gateway/contract (DexFamily).
//! Настройки: dex.universal_router, dex.permit2, dex.universal_router_version [DEX]; smart.universal_router,
//! smart.permit2 [bsc_dex]; через classify — dex.kind, dex.pool_enabled и адреса.

use crate::gateway::contract::DexFamily;

use super::super::ctx::Ctx;
use super::families::{Family, all, classify};

/// Исполняет ли пулы этой DEX умный роутер (решение 38): её dex.universal_router — это smart.universal_router, формат
/// команд — pancakeswap_infinity и dex.permit2 = smart.permit2. Ошибок нет: не задано — нет.
pub fn smart_ok(ctx: &Ctx, f: &Family) -> bool {
    let same = |a: &str, b: &str| match (ctx.dex_addr(a, f.id), ctx.addr(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    };
    let format = ctx.dex_text("dex.universal_router_version", f.id);
    f.tradable
        && same("dex.universal_router", "smart.universal_router")
        && same("dex.permit2", "smart.permit2")
        && format.as_deref().is_ok_and(|v| v == "pancakeswap_infinity")
}

/// Пул по-прежнему того вида, с которым он взят: DEX подключена и её вид в таблице не менялся (dex.kind и адреса
/// меняются на лету — пул старого вида не котируется до пересборки подписки). Ошибка — причина.
pub fn still(ctx: &Ctx, f: &'static Family) -> Result<(), String> {
    let now = classify(ctx, f.id)?;
    if std::ptr::eq(now, f) {
        Ok(())
    } else {
        Err(format!(
            "вид DEX {} в таблице изменился — пул старого вида не котируется до пересборки подписки",
            f.id
        ))
    }
}

/// Список для контракта (dex_families): DEX с поддержанным видом из настроек. Ошибок нет.
pub fn for_ui(ctx: &Ctx) -> Vec<DexFamily> {
    all(ctx)
        .into_iter()
        .map(|f| DexFamily {
            dex: f.id.to_string(),
            family: f.id.to_string(),
            kind: f.kind,
            title: f.title.to_string(),
            docs: Vec::new(),
        })
        .collect()
}
