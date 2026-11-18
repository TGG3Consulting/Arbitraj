//! Файл: venues/bsc_dex/exec/presell_credit.rs
//! Что делает: решение 407 (задача t192) — симуляция продажи до покупки с балансом монеты кошелька подменой состояния
//! (слот баланса — exec/presell_slot.rs). Порядок: настройка presell.balance_slot_scan задана → слот найден достоверно
//! → exec/dry_sim.rs с Io.credit (слот баланса кошелька = нужному количеству; у кошелька уже хватает — подмены нет,
//! как раньше). balanceOf входа после подмены ≠ нужному ровно (Sim.credit_bad: раскладка изменилась) — слот забыт,
//! симуляция повторяется без подмены. Слот не найден / поиск не выполнен / подмена недостоверна — путь как раньше
//! (пополнение exec/dry_fund.rs, при нехватке — Short решения 380), а причина дописывается к «монеты в симуляции»
//! (Sim.funding) и к причине отказа (она же — текст Short и строка урезания 380). Не задана — 1 в 1 как до решения.
//! Налог токена при продаже симуляция видит как раньше: подменяется только слот баланса кошелька, код токена и прочие
//! его слоты — как в сети (продажа идёт обычным transfer кошелька в пул).
//! Кто вызывает: exec/presell.rs.
//! От чего зависит: exec/{dry_sim, dry_build (Ready), presell_slot}.rs, check/simulate.rs (Override).
//! Настройки: presell.balance_slot_scan [bsc_dex] (через presell_slot).

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;

use super::super::check::simulate::Override;
use super::super::quote::Quoting;
use super::dry_build::Ready;
use super::dry_sim::{self, Io, Sim};
use super::presell_slot;

/// Симуляция продажи до покупки (см. шапку). Принимает: котировку, сделку, кошелёк, вход (Io.credit не учитывается —
/// подмена строится здесь). Возвращает: Sim (с пометкой решения 407, если подмена не применена). Ошибки: как у
/// dry_sim::run.
pub async fn run(q: &Quoting, r: &Ready, wallet: Address, io: &Io<'_>) -> GatewayResult<Sim> {
    let (credit, mut note) = prepare(q, r, wallet, io).await;
    let with = Io { credit, ..*io };
    let mut sim = dry_sim::run(q, r, wallet, &with).await?;
    if sim.credit_bad {
        let why = sim.reason.take().unwrap_or_default();
        io.token_in.into_iter().for_each(presell_slot::forget);
        note = Some(format!("{why} — повтор без подмены (решение 407)"));
        sim = dry_sim::run(
            q,
            r,
            wallet,
            &Io {
                credit: None,
                ..*io
            },
        )
        .await?;
    }
    if let Some(n) = note {
        sim.reason = sim.reason.map(|w| format!("{w}; {n}"));
        sim.funding = Some(match sim.funding.take() {
            Some(f) => format!("{f}; {n}"),
            None => n,
        });
    }
    Ok(sim)
}

/// Подмена баланса входа. Принимает: котировку, сделку (блок r.at), кошелёк, вход. Возвращает: (подмена — слот
/// найден; пометка — почему подмены нет при заданной настройке). Ошибок нет: сбой настройки или поиска — пометка.
async fn prepare(
    q: &Quoting,
    r: &Ready,
    wallet: Address,
    io: &Io<'_>,
) -> (Option<Override>, Option<String>) {
    let Some(token) = io.token_in else {
        return (None, None);
    };
    let n = match presell_slot::scan(&q.ctx) {
        Ok(Some(n)) => n,
        Ok(None) => return (None, None),
        Err(e) => {
            let why = format!("{}: {} (решение 407)", presell_slot::SETTING, e.message);
            return (None, Some(why));
        }
    };
    match presell_slot::find(&q.ctx, token, wallet, io.need, r.at, n).await {
        Ok(Some(s)) => (Some(presell_slot::credit(token, wallet, s, io.need)), None),
        Ok(None) => (
            None,
            Some(format!(
                "подмена баланса не применена: balanceOf не вернул подставленное ни в одном из слотов 0..{} \
                 (раскладки Solidity и Vyper; рефлексия, ребейз или иное хранение) — источники, как раньше \
                 (решение 407)",
                n - 1
            )),
        ),
        Err(e) => (
            None,
            Some(format!(
                "поиск слота баланса не выполнен: {} — источники, как раньше (решение 407)",
                e.message
            )),
        ),
    }
}
