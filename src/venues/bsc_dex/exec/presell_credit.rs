//! Файл: venues/bsc_dex/exec/presell_credit.rs
//! Что делает: решение 407 (задача t192) — симуляция продажи до покупки с балансом монеты кошелька подменой состояния
//! (слот баланса — exec/presell_slot.rs, поиск — exec/presell_slot_find.rs). Порядок: настройка
//! presell.balance_slot_scan задана → баланс входа кошелька на блоке симуляции (ревью Н2: у кошелька хватает — поиска
//! нет, баланс передаётся в dry_sim через Io.have, второго чтения нет) → слот найден и проверен → слово слота = балансу
//! (ревью М5: не упакован) → exec/dry_sim.rs с Io.credit (слот баланса кошелька = нужному количеству).
//! balanceOf входа после подмены ≠ нужному ровно (Sim.credit_bad) — токен помечен недостоверным (ревью Н1: не искать
//! снова на каждой предпроверке), симуляция повторяется без подмены. Слот в кэше — баланс и слово слота читаются
//! параллельно (ревью М-a). Слот не найден / поиск не выполнен / подмена недостоверна — путь как раньше (пополнение
//! exec/dry_fund.rs, при нехватке — Short решения 380), а причина дописывается к «монеты в симуляции»
//! (Sim.funding) и к причине отказа (она же — текст Short и строка урезания 380). Не задана — 1 в 1 как до решения.
//! Налог токена при продаже симуляция видит как раньше: подменяется только слот баланса кошелька, код токена и прочие
//! его слоты — как в сети (продажа идёт обычным transfer кошелька в пул).
//! Кто вызывает: exec/presell.rs.
//! От чего зависит: exec/{dry_sim, dry_build (Ready), presell_slot, presell_slot_find}.rs,
//! check/simulate.rs (Override), node/{call, abi}.rs (balanceOf на блоке).
//! Настройки: presell.balance_slot_scan [bsc_dex] (через presell_slot).

use alloy_primitives::{Address, U256};

use crate::gateway::errors::GatewayResult;

use super::super::check::simulate::Override;
use super::super::node::abi::IERC20;
use super::super::node::call;
use super::super::quote::Quoting;
use super::dry_build::Ready;
use super::dry_sim::{self, Io, Sim};
use super::{presell_slot, presell_slot_find as find};

/// Подготовка симуляции: подмена (слот найден), пометка (почему подмены нет), прочитанный баланс входа.
struct Prep {
    credit: Option<Override>,
    note: Option<String>,
    have: Option<U256>,
    n: u64,
    warm: bool,
    stamp: Option<super::presell_warm::Stamp>,
}

/// Симуляция продажи до покупки (см. шапку). Принимает: котировку, сделку, кошелёк, вход (Io.credit и Io.have
/// строятся здесь). Возвращает: Sim (с пометкой решения 407, если подмена не применена). Ошибки: как у dry_sim::run
/// (и узел — баланс входа на блоке симуляции).
pub async fn run(q: &Quoting, r: &Ready, wallet: Address, io: &Io<'_>) -> GatewayResult<Sim> {
    let p = prepare(q, r, wallet, io).await?;
    let mut note = p.note;
    let with = Io {
        credit: p.credit,
        have: p.have,
        ..*io
    };
    let attempt = dry_sim::run_anchored(q, r, wallet, &with, p.stamp.map(|s| s.hash)).await;
    let mut sim = match attempt {
        Ok(value) => value,
        Err(e) if p.stamp.is_some() => {
            // Блок мог быть заменён. Сохраняем причину и повторяем без старого баланса.
            Sim {
                credit_bad: true,
                reason: Some(e.message),
                ..Default::default()
            }
        }
        Err(e) => return Err(e),
    };
    if sim.credit_bad {
        let why = sim.reason.take().unwrap_or_default();
        if let Some(t) = io.token_in {
            if let Some(stamp) = p.stamp {
                q.ctx.evm_chain.presell().reject(wallet, t, stamp, &why);
            } else {
                find::reject(q.ctx.place.id, t, p.n, &why);
            }
        }
        note = Some(format!("{why} — повтор без подмены (решение 407)"));
        let plain = Io {
            credit: None,
            have: if let (Some(_), Some(t)) = (p.stamp, io.token_in) {
                Some(
                    call::view(
                        &q.ctx,
                        t,
                        &IERC20::balanceOfCall { owner: wallet },
                        &call::block_tag(r.at.number),
                    )
                    .await?,
                )
            } else {
                p.have
            },
            ..*io
        };
        sim = dry_sim::run(q, r, wallet, &plain).await?;
    }
    // Пока dry_fund проверял источники, фон мог подготовить полный объём.
    // Не превращаем такой исход в урезание 380 и не ждём незавершённый поиск.
    if p.n > 0
        && p.stamp.is_none()
        && sim.fundable.is_some()
        && super::presell_warm::applies(&q.ctx)
        && let Some(t) = io.token_in
        && let Ok((stamp, field, have)) =
            q.ctx
                .evm_chain
                .presell()
                .read(wallet, t, r.at.number, q.ctx.settings.generation())
    {
        let retry = Io {
            credit: (have < io.need).then(|| field.credit(io.need)),
            have: Some(have),
            ..*io
        };
        match dry_sim::run_anchored(q, r, wallet, &retry, Some(stamp.hash)).await {
            Ok(result) if !result.credit_bad => {
                q.ctx.evm_chain.presell().funding_short(true);
                sim = result;
                note=Some("фоновая проверка завершилась во время проверки источников; проверен полный объём".into());
            }
            Ok(result) => q.ctx.evm_chain.presell().reject(
                wallet,
                t,
                stamp,
                &result.reason.unwrap_or_default(),
            ),
            Err(e) => q
                .ctx
                .evm_chain
                .presell()
                .reject(wallet, t, stamp, &e.message),
        }
    }
    if p.warm && sim.fundable.is_some() {
        q.ctx.evm_chain.presell().funding_short(false);
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

/// Подмена баланса входа (см. шапку). Принимает: котировку, сделку (блок r.at), кошелёк, вход. Возвращает: Prep;
/// настройка не задана — всё пусто (dry_sim как до решения). Ошибки: узел (баланс входа); сбой настройки, поиска или
/// чтения слова слота — пометка, не ошибка.
async fn prepare(q: &Quoting, r: &Ready, wallet: Address, io: &Io<'_>) -> GatewayResult<Prep> {
    let mut p = Prep {
        credit: None,
        note: None,
        have: None,
        n: 0,
        warm: false,
        stamp: None,
    };
    let Some(token) = io.token_in else {
        return Ok(p);
    };
    p.n = match presell_slot::scan(&q.ctx) {
        Ok(Some(n)) => n,
        Ok(None) => return Ok(p),
        Err(e) => {
            p.note = Some(format!(
                "{}: {} (решение 407)",
                presell_slot::SETTING,
                e.message
            ));
            return Ok(p);
        }
    };
    let tag = call::block_tag(r.at.number);
    let owner = IERC20::balanceOfCall { owner: wallet };
    let block = r.at.number;
    // Ревью М-a: слот уже в кэше — баланс и слово слота читаются параллельно (круга до узла не добавляется).
    if super::presell_warm::applies(&q.ctx) {
        let record =
            q.ctx
                .evm_chain
                .presell()
                .read(wallet, token, block, q.ctx.settings.generation());
        p.warm = true;
        match record {
            Ok((stamp, field, have)) => {
                p.stamp = Some(stamp);
                p.have = Some(have);
                if have < io.need {
                    p.credit = Some(field.credit(io.need));
                    p.note = Some(q.ctx.evm_chain.presell().totals());
                }
            }
            Err(reason) => {
                p.have = Some(call::view(&q.ctx, token, &owner, &tag).await?);
                p.note = Some(format!(
                    "{reason} — штатное пополнение (решение 407), {}",
                    q.ctx.evm_chain.presell().totals()
                ));
            }
        }
        return Ok(p);
    }
    let known = find::cached(q.ctx.place.id, token);
    let (have, raw) = match known {
        Some(s) => {
            let (h, w) = tokio::join!(
                call::view(&q.ctx, token, &owner, &tag),
                find::word(&q.ctx, (token, wallet, s), block)
            );
            (h?, Some(w))
        }
        None => (call::view(&q.ctx, token, &owner, &tag).await?, None),
    };
    p.have = Some(have);
    if have >= io.need {
        return Ok(p);
    }
    let checked = match known {
        Some(s) => Ok(s),
        None => find::find(&q.ctx, token, wallet, io.need, r.at, p.n).await,
    };
    let checked = match (checked, raw) {
        (Ok(s), Some(w)) => w.and_then(|w| find::unpacked(w, have)).map(|()| s),
        (Ok(s), None) => find::word(&q.ctx, (token, wallet, s), block)
            .await
            .and_then(|w| find::unpacked(w, have))
            .map(|()| s),
        (Err(why), _) => Err(why),
    };
    match checked {
        Ok(s) => p.credit = Some(presell_slot::credit(token, wallet, s, io.need)),
        Err(why) => p.note = Some(format!("{why} — источники, как раньше")),
    }
    Ok(p)
}
