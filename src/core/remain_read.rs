//! Файл: core/remain_read.rs
//! Что делает: решение 413 (вынесено из remain_fact.rs; повторное ревью t198, Б-3) — чтение факта счёта сделки для
//! сверки остатка и доли: баланс монеты (биржа — ТОЛЬКО торговый счёт (спот), free + locked; DEX —
//! кошелёк сделки) и учёт других сделок — одним снимком под замком счёта (credit_lock.rs, ждёт конца продаж). Сбой
//! чтения с кодом шлюза повторяется только по правилу «повторить» для этого кода, без побочных действий правил шлюза
//! (третье ревью t198, С-З: чтение баланса — не денежная операция); сбой без кода — сразу ошибка. Итог на бирже — одна
//! строка Warn core.credit_fact; пока не читается — пауза новых входов на биржу, первое успешное чтение её снимает
//! (fact_pause.rs, третье ревью С-Д).
//! Кто вызывает: core/remain_fact.rs (reconcile), core/credit_base.rs и core/credit_apply.rs (fact_at).
//! От чего зависит: credit_lock.rs, remain_others.rs, deposit_proof_plan.rs (addr_retry), gateway/rules.rs,
//! storage/rules.rs, fact_pause.rs, dex_account.rs, manual_halted.rs (free_of), venue_ticker.rs, engine.rs.
//! Настройки: правила шлюза (только «повторить»).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::gateway::contract::{AccountRole, VenueKind};
use crate::gateway::errors::GatewayError;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::remain_others::Others;

/// Что делает: факт счёта сделки и учёт других под замком (ждёт конца продаж на счёте — третье ревью, С-А), с
/// повторами по правилу шлюза «повторить» для кода ошибки (шапка). Итог на бирже: прочитано — снять паузу «баланс биржи
/// не читается» (fact_pause.rs), не прочитано — одна строка Warn core.credit_fact и пауза новых входов на биржу.
/// Принимает: ядро, сделку. Возвращает: (факт или текст ошибки, другие). Ошибок нет.
pub async fn read(core: &Core, deal: &Deal) -> (Result<Decimal, String>, Others) {
    Box::pin(read_with(core, deal, true)).await
}

/// Что делает: шестое ревью t198 (Б-2), седьмое (мелкое 3) — чтение факта и учёта других и решение по ним `f` под ОДНИМ
/// замком счёта (чтение и запись решения в учёт памяти — без окна для продажи другой сделки). Как read_with: повторы
/// только по правилу «повторить» для кода (между попытками замок отпущен), итог на бирже снимает или ставит паузу
/// «баланс не читается» (fact_pause.rs); строки Warn нет (пишет вызывающий). Принимает: ядро, сделку (для чтения),
/// решение. Возвращает: итог решения. Ошибок нет.
pub async fn with_fact<R>(
    core: &Core,
    deal: &Deal,
    f: impl FnOnce(Result<Decimal, String>, Others) -> R,
) -> R {
    let cex = super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let g = super::credit_lock::fact(&deal.at, &deal.direction.asset).await;
        let (c, d) = (core.clone(), deal.clone());
        let r = match tokio::spawn(async move { fact_try(&c, &d, &d.at).await }).await {
            Ok(r) => r,
            Err(e) => Err((format!("{}: чтение баланса прервано: {e}", deal.at.0), None)),
        };
        let (text, ge) = match r {
            Ok(v) => {
                let o = super::remain_others::on_account(core, deal, &deal.at);
                let out = f(Ok(v), o);
                drop(g);
                if cex {
                    Box::pin(super::fact_pause::lift(core, &deal.at)).await;
                }
                return out;
            }
            Err(x) => x,
        };
        drop(g);
        let pause = match &ge {
            Some(e) => Box::pin(retry_pause(core, &deal.at, e, attempt)).await,
            None => None,
        };
        if let Some(ms) = pause {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            continue;
        }
        let g = super::credit_lock::fact(&deal.at, &deal.direction.asset).await;
        let o = super::remain_others::on_account(core, deal, &deal.at);
        let out = f(Err(text.clone()), o);
        drop(g);
        if cex {
            Box::pin(super::fact_pause::set(core, &deal.at, &text)).await;
        }
        return out;
    }
}

/// Что делает: как read; `warn` = false — без строки Warn на итог (четвёртое ревью t198, С-4: повторные чтения
/// ожидания первого вывода — Warn только при первом сбое). Принимает: ядро, сделку, писать ли Warn. Возвращает: как
/// read. Ошибок нет.
pub async fn read_with(core: &Core, deal: &Deal, warn: bool) -> (Result<Decimal, String>, Others) {
    let cex = super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex);
    let mut attempt = 0;
    loop {
        attempt += 1;
        let g = super::credit_lock::fact(&deal.at, &deal.direction.asset).await;
        // Чтение — своей задачей tokio (CI 3f1905ce, переполнение стека): HTTP-будущие адаптера (у части бирж — несколько
        // счетов параллельно) опрашиваются на стеке своей задачи, а не в глубине пути продажи (решение 389).
        let (c, d) = (core.clone(), deal.clone());
        let r = match tokio::spawn(async move { fact_try(&c, &d, &d.at).await }).await {
            Ok(r) => r,
            Err(e) => Err((format!("{}: чтение баланса прервано: {e}", deal.at.0), None)),
        };
        let o = super::remain_others::on_account(core, deal, &deal.at);
        drop(g);
        match r {
            Ok(f) => {
                if cex {
                    Box::pin(super::fact_pause::lift(core, &deal.at)).await;
                }
                return (Ok(f), o);
            }
            Err((text, ge)) => {
                // Третье ревью (С-З): чтение баланса — не денежная операция: только правило «повторить» для кода
                // (deposit_proof_plan::addr_retry — тот же чистый выбор, что у адреса депозита, решение 411), без
                // побочных действий правил шлюза (журнал Error на попытку, Telegram, ошибки подряд, «стоп», «исход
                // неизвестен»).
                let pause = match &ge {
                    Some(e) => Box::pin(retry_pause(core, &deal.at, e, attempt)).await,
                    None => None,
                };
                if let Some(ms) = pause {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    continue;
                }
                if cex && warn {
                    let t = format!(
                        "{}: баланс не прочитан после {attempt} попыток: {text}",
                        deal.id
                    );
                    Box::pin(core.inner.deps.log(
                        Level::Warn,
                        "core.credit_fact",
                        Some(&deal.at.0),
                        &t,
                        None,
                    ))
                    .await;
                }
                if cex {
                    Box::pin(super::fact_pause::set(core, &deal.at, &text)).await;
                }
                return (Err(text), o);
            }
        }
    }
}

/// Пауза повтора по правилу шлюза «повторить» для кода ошибки (без побочных действий). None — не повторять.
async fn retry_pause(core: &Core, v: &VenueId, e: &GatewayError, attempt: u32) -> Option<u64> {
    let rules = crate::storage::rules::list(&core.inner.deps.db)
        .await
        .unwrap_or_default();
    let rule = crate::gateway::rules::pick_rule(&rules, e.code, Some(v)).map(|r| &r.action);
    super::deposit_proof_plan::addr_retry(rule, e.code, attempt)
}

/// Фактическое количество монеты сделки на площадке `v` (и до вывода туда — credit_base.rs). Ошибки — текст.
pub async fn fact_at(core: &Core, deal: &Deal, v: &VenueId) -> Result<Decimal, String> {
    fact_try(core, deal, v).await.map_err(|e| e.0)
}

/// Что делает: факт на площадке; ошибка — текст и ошибка шлюза (если она с кодом). Ошибок нет, кроме возвращаемой.
async fn fact_try(
    core: &Core,
    deal: &Deal,
    v: &VenueId,
) -> Result<Decimal, (String, Option<Box<GatewayError>>)> {
    let a = &deal.direction.asset;
    let gw = super::dex_account::venue_of(core, deal, v)
        .await
        .map_err(|e| (e, None))?;
    if gw.kind() != VenueKind::Cex {
        return super::manual_halted::free_of(core, gw.as_ref(), v, a)
            .await
            .map_err(|e| (e, None));
    }
    let on = super::venue_ticker::ticker_on(core, a, v);
    let accs = gw.account_balances().await.map_err(|e| {
        let t = format!("{}: балансы счетов: {}", v.0, e.message);
        (t, Some(Box::new(e)))
    })?;
    let mut sum = Decimal::ZERO;
    // Решение Тиграна 21.11.2026: только торговый счёт (спот). Фонд (Funding) и прочие счета не читаем и не считаем —
    // ошибка чтения Фонда не должна рушить факт сделки и ставить паузу бирже (везде используется спот).
    for acc in accs
        .iter()
        .filter(|x| gw.account_role(&x.account) == AccountRole::Trading)
    {
        if let Some(e) = &acc.error {
            let t = format!("{}: счёт «{}» не прочитан: {e}", v.0, acc.account);
            return Err((t, None));
        }
        sum += acc
            .balances
            .iter()
            .filter(|b| b.asset == on)
            .map(|b| b.free + b.locked)
            .sum::<Decimal>();
    }
    Ok(sum)
}
