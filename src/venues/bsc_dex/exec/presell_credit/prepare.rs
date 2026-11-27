//! Передаёт в симуляцию проверенное на её блоке количество токенов.
//! При отсутствии достоверного поля сохраняет причину и реальный баланс кошелька.
use super::super::super::check::simulate::Override;
use super::super::{presell_slot, presell_warm};
use super::{Address, GatewayResult, IERC20, Io, Quoting, Ready, call, find};
use alloy_primitives::U256;

/// Подготовка симуляции: подмена (слот найден), пометка (почему подмены нет), прочитанный баланс входа.
pub(super) struct Prep {
    pub(super) credit: Option<Override>,
    pub(super) note: Option<String>,
    pub(super) have: Option<U256>,
    pub(super) n: u64,
    pub(super) warm: bool,
    pub(super) stamp: Option<presell_warm::Stamp>,
}

/// Подмена баланса входа (см. шапку). Принимает: котировку, сделку (блок r.at), кошелёк, вход. Возвращает: Prep;
/// настройка не задана — всё пусто (dry_sim как до решения). Ошибки: узел (баланс входа); сбой настройки, поиска или
/// чтения слова слота — пометка, не ошибка.
pub(super) async fn prepare(
    q: &Quoting,
    r: &Ready,
    wallet: Address,
    io: &Io<'_>,
) -> GatewayResult<Prep> {
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
    if presell_warm::applies(&q.ctx) {
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
                    p.credit = field.credit(io.need);
                    p.note = Some(if p.credit.is_some() {
                        q.ctx.evm_chain.presell().totals()
                    } else {
                        format!(
                            "объём превышает uint{} баланса — штатное пополнение",
                            field.max.bit_len()
                        )
                    });
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
