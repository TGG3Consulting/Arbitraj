//! Файл: core/to_trading.rs
//! Что делает: решение 174 — ОДИН шаг «монета не на торговом счёте биржи → перевести на торговый» для всех продаж по
//! кнопке человека: «Продать» панели «Балансы» (balance_sell.rs, счёт указан строкой панели) и «Продать сейчас» /
//! «Продать и перегнать» (manual_halted.rs: on_hand_sell — монеты сделки ищутся на счетах роли ToTrading). Живой случай
//! 05.11 (сделка 20261105020010981ION): 905432 ION на KuCoin лежали на main («Фонд (Funding)»), «Продать сейчас» дважды
//! ответил «нет свободных ION», потому что смотрел только торговый счёт.
//! Перевод — методом контракта move_to_trading (роль счёта — account_role); у биржи перевода нет — отказ с причиной.
//! Кто вызывает: core/balance_sell.rs, core/manual_halted.rs.
//! От чего зависит: gateway (контракт Venue: account_role, account_balances, balance, move_to_trading).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::Asset;
use crate::gateway::contract::{AccountRole, Venue, VenueKind};

/// Перевод `qty` монеты со счёта `account` на торговый. Принимает: адаптер, счёт (название из account_balances),
/// монету, количество. Возвращает: текст перевода от площадки. Ошибки: счёт не той роли, отказ площадки.
pub async fn move_from(
    gw: &dyn Venue,
    account: &str,
    a: &Asset,
    qty: Decimal,
) -> Result<String, String> {
    let v = gw.id();
    if gw.account_role(account) != AccountRole::ToTrading {
        return Err(format!(
            "перевода со счёта «{account}» на торговый в адаптере {} нет — переведите вручную",
            v.0
        ));
    }
    gw.move_to_trading(a, qty).await.map_err(|e| {
        format!(
            "перевод {qty} {} «{account}» → торговый счёт {} не прошёл: {}",
            a.ticker, v.0, e.message
        )
    })
}

/// Добрать монету на торговый счёт биржи до `want` со счетов роли ToTrading. Принимает: адаптер, монету, сколько нужно
/// на торговом. Возвращает: None — переводить не нужно или нечего (DEX, хватает, на других счетах монеты нет);
/// Some(текст переводов). Ошибки: только когда на торговом счёте монеты нет совсем, а балансы счетов не прочитаны или
/// перевод не прошёл (есть хоть сколько-то — сбой перевода уходит в текст, продаётся то, что есть).
pub async fn top_up(gw: &dyn Venue, a: &Asset, want: Decimal) -> Result<Option<String>, String> {
    if gw.kind() != VenueKind::Cex {
        return Ok(None);
    }
    let v = gw.id();
    let have = gw
        .balance(&a.ticker)
        .await
        .map_err(|e| format!("{}: баланс торгового счёта: {}", v.0, e.message))?
        .map_or(Decimal::ZERO, |b| b.free);
    if have >= want {
        return Ok(None);
    }
    let soft = |e: String| {
        if have > Decimal::ZERO {
            Ok(Some(e))
        } else {
            Err(e)
        }
    };
    let accs = match gw.account_balances().await {
        Ok(x) => x,
        Err(e) => {
            return soft(format!(
                "{}: балансы счетов не прочитаны: {}",
                v.0, e.message
            ));
        }
    };
    let mut left = want - have;
    let mut done = Vec::new();
    for acc in accs
        .iter()
        .filter(|x| gw.account_role(&x.account) == AccountRole::ToTrading)
    {
        let free = acc
            .balances
            .iter()
            .find(|b| b.asset == a.ticker)
            .map_or(Decimal::ZERO, |b| b.free);
        let take = free.min(left);
        if take <= Decimal::ZERO {
            continue;
        }
        match move_from(gw, &acc.account, a, take).await {
            Ok(t) => {
                done.push(t);
                left -= take;
            }
            Err(e) if done.is_empty() => return soft(e),
            Err(e) => done.push(e),
        }
        if left <= Decimal::ZERO {
            break;
        }
    }
    Ok((!done.is_empty()).then(|| done.join("; ")))
}
