//! Файл: venues/bsc_dex/check/tax_wallet.rs
//! Что делает: каким кошельком проверять налоги монеты в цикле налогов. Дополнение 10.11 к решениям 247/252 (Тигран:
//! согласовано; задача t77): перепроверка после отката продажи выхода «мало на выходе» (exec/executor/remeasure.rs,
//! exec/ur_remeasure.rs → check/recheck.rs coins_by) — кошельком сделки, которым идёт продажа (счёт DEX сделки,
//! решения 132, 220). Факт t71: кошелёк строки «Пробы» (signer::address без счёта) и кошелёк сделки (например, w2) —
//! разные адреса; налог reflection- и фи-токенов может зависеть от отправителя и его баланса. Остальные проверки
//! (полный проход, «Перепроверить», «пул готов») — кошельком строки «Пробы», как раньше: обращений к узлу не больше —
//! та же одна проверка монеты, только от другого адреса.
//! Счёт помнится, пока проверка не дала итога (прошла / не прошла) или не упёрлась в настройку (например, кошелька
//! сделки больше нет): ожидание (сбой узла, пулы грузятся) повторяется тем же кошельком сделки.
//! Кто вызывает: check/tax_cycle.rs (счета из очереди, снятие после итога), check/tax_pass.rs (окружение и журнал).
//! От чего зависит: ctx.rs, ctx_wallet.rs (with_account, wallet_name), check/tax_pass.rs (Res), check/taxes.rs (Outcome).
//! Настроек нет.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::tax_pass::Res;
use super::taxes::Outcome;

/// Окружение проверки монеты: привязанное к счёту сделки, если монета ждёт перепроверки его кошельком; иначе None —
/// окружение цикла (кошелёк строки «Пробы»). Принимает: окружение цикла, счета по монетам, монету. Ошибок нет.
pub fn ctx_for(base: &Ctx, accounts: &HashMap<Address, String>, coin: &Address) -> Option<Ctx> {
    accounts.get(coin).map(|id| base.with_account(id))
}

/// Снимает счёт у монет, проверка которых дала итог или упёрлась в настройку (повтор тем же кошельком не нужен).
/// Принимает: счета по монетам, итоги прохода (tax_pass::Pass::done). Ошибок нет.
pub fn settle(accounts: &mut HashMap<Address, String>, done: &[(Address, Res)]) {
    for (a, r) in done {
        if matches!(r, Res::Ok | Res::Failed | Res::Config) {
            accounts.remove(a);
        }
    }
}

/// Строка журнала dex.tax_recheck: перепроверка кошельком сделки и её итог. Принимает: окружение счёта сделки, тикер,
/// итог. Ошибок нет.
pub async fn log(ctx: &Ctx, ticker: &str, out: &Outcome) {
    let what = match out {
        Outcome::Known(t) => format!(
            "налог покупки {}%, продажи {}%, перевода {}%",
            t.buy_pct.round_dp(4).normalize(),
            t.sell_pct.round_dp(4).normalize(),
            t.transfer_pct.round_dp(4).normalize()
        ),
        Outcome::Failed(why) => format!("проверка не прошла: {why}"),
        Outcome::Wait(why) => format!("не проверено (повтор тем же кошельком): {why}"),
        Outcome::Config(why, _) => format!("не задана настройка: {why}"),
    };
    let msg = format!(
        "{ticker}: перепроверка налогов кошельком сделки «{}» (после отката продажи выхода) — {what}",
        ctx.wallet_name()
    );
    ctx.log(Level::Info, "dex.tax_recheck", msg, None).await;
}
