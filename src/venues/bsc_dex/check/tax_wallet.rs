//! Файл: venues/bsc_dex/check/tax_wallet.rs
//! Что делает: перепроверка налогов монеты кошельком сделки в цикле налогов. Дополнение 10.11 к решениям 247/252
//! (Тигран: согласовано; задача t77): после отката продажи выхода «мало на выходе» (exec/executor/remeasure.rs,
//! exec/ur_remeasure.rs → check/recheck.rs coins_by) монета проверяется кошельком сделки, которым идёт продажа (счёт DEX
//! сделки, решения 132, 220). Факт t71: кошелёк строки «Пробы» (signer::address без счёта) и кошелёк сделки (например,
//! w2) — разные адреса; налог reflection- и фи-токенов может зависеть от отправителя и его баланса.
//! t78 (дополнение 10.11, Тигран: согласовано): итог кошелька сделки — ОТДЕЛЬНО, только для продажи этой сделки
//! (check/deal_taxes.rs: пробы пишут в пустое хранилище, из него — налоги пулов сделки); общие налоги монеты и пулов,
//! событие CoinCheck ядру — только от кошелька строки «Пробы» (та же монета в том же проходе проверяется и им, как до
//! t77). Обращений к узлу у такой монеты — две проверки вместо одной, только после отката продажи выхода.
//! Кошелёк сделки помнится, пока его проверка не дала итога (прошла / не прошла) или не упёрлась в настройку (например,
//! кошелька сделки больше нет): ожидание (сбой узла) повторяется тем же кошельком сделки.
//! Кто вызывает: check/tax_cycle.rs (кошельки из очереди, снятие после итога, повтор ожидания), check/tax_pass.rs
//! (проверка кошельком сделки).
//! От чего зависит: ctx.rs, ctx_wallet.rs (with_account, wallet_name), check/taxes.rs (check_coin, Outcome),
//! check/state.rs (Checks), check/deal_taxes.rs (DealTax), check/recheck.rs (By), check/tax_pass.rs (Res, Why).
//! Настроек нет.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::pools::listen::Pools;
use super::deal_taxes::DealTax;
use super::recheck::By;
use super::state::Checks;
use super::tax_pass::{Res, Why};
use super::taxes::{self, Outcome};
use super::taxes_util::Sources;

/// Проверяет монету кошельком сделки; итог (прошла / не прошла) — в налоги этой сделки (Checks::put_deal), общие
/// налоги не трогаются. Принимает: окружение цикла, источники пробы прохода, кошелёк сделки, тикер, адрес монеты, USDT.
/// Возвращает: вид итога (Wait — повтор тем же кошельком; Config — кошелёк снимается). Ошибок нет.
pub async fn deal_check(
    p: &Pools,
    sources: &Sources,
    by: &By,
    ticker: &str,
    coin: Address,
    usdt: Address,
) -> Res {
    let ctx = p.ctx.with_account(&by.account);
    // Пустое хранилище: в нём — только то, что намерила эта проверка (общие итоги не меняются).
    let own = Checks::default();
    let (c, out) = taxes::check_coin(
        &ctx, &p.reg, &own, &p.tokens, sources, ticker, coin, usdt, &p.heads,
    )
    .await;
    log(&ctx, &by.deal, ticker, &out).await;
    let res = match out {
        Outcome::Known(_) => Res::Ok,
        Outcome::Failed(_) => Res::Failed,
        Outcome::Wait(_) => Res::Wait,
        Outcome::Config(..) => Res::Config,
    };
    if matches!(res, Res::Ok | Res::Failed) {
        let t = DealTax {
            account: by.account.clone(),
            check: c,
            pools: own.pool_taxes(),
        };
        p.checks.put_deal(&by.deal, coin, t);
    }
    res
}

/// Снимает кошелёк сделки у монет, его проверка которых дала итог или упёрлась в настройку (повтор не нужен).
/// Принимает: кошельки по монетам, итоги проверок кошельком сделки (tax_pass::Pass::deal_done). Ошибок нет.
pub fn settle(accounts: &mut HashMap<Address, By>, done: &[(Address, Res)]) {
    for (a, r) in done {
        if matches!(r, Res::Ok | Res::Failed | Res::Config) {
            accounts.remove(a);
        }
    }
}

/// Проверка кошельком сделки ждёт (сбой узла), а монета не в списке ждущих (общая проверка дала итог) — монета в
/// ждущие со сбоем узла: повтор через tax.retry_s (тем же кошельком сделки, пока он помнится). Принимает: ждущие,
/// итоги кошелька сделки, монеты прохода (адрес, тикер). Ошибок нет.
pub fn wait_more(
    waiting: &mut Vec<(Address, String, Why)>,
    done: &[(Address, Res)],
    list: &[(Address, String)],
) {
    for (a, r) in done {
        if *r != Res::Wait || waiting.iter().any(|w| w.0 == *a) {
            continue;
        }
        if let Some((_, t)) = list.iter().find(|(x, _)| x == a) {
            waiting.push((*a, t.clone(), Why::Node));
        }
    }
}

/// Строка журнала dex.tax_recheck: перепроверка кошельком сделки и её итог. Принимает: окружение счёта сделки, id
/// сделки, тикер, итог. Ошибок нет.
pub async fn log(ctx: &Ctx, deal: &str, ticker: &str, out: &Outcome) {
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
        "{deal} {ticker}: перепроверка налогов кошельком сделки «{}» (после отката продажи выхода; итог — только для продажи этой сделки) — {what}",
        ctx.wallet_name()
    );
    ctx.log(Level::Info, "dex.tax_recheck", msg, None).await;
}
