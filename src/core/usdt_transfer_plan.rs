//! Файл: core/usdt_transfer_plan.rs
//! Что делает: решение 176 — проверка и сводка ручного перевода USDT «с X на Y» до отправки: концы перевода (биржа или
//! кошелёк DEX по маппингу, решение 132), сеть BSC, комиссия и минимум вывода из справочника сетей биржи (данные биржи
//! из её API), минимум депозита получателя, свободно на отправителе (один запрос баланса — как перед покупкой, limits.rs
//! free_now) и «можно вывести» (решение 166: удержания биржи, precheck_withdraw::held). Та же проверка — у сводки в
//! Telegram и в интерфейсе и у самой команды перед отправкой (usdt_transfer.rs): одна логика.
//! Кошелёк ↔ кошелёк не переводится: у ядра нет такого пути (перевод ядра — биржа ↔ биржа и биржа ↔ кошелёк).
//! Типы и чистая проверка суммы — usdt_transfer_check.rs.
//! Кто вызывает: core/usdt_transfer.rs (команда), api/usdt_transfer.rs (сводка интерфейса), notify/ctl/transfer*.rs.
//! От чего зависит: usdt_return.rs (USDT как актив), transfers.rs (сеть BSC), costs_withdraw.rs (точность вывода),
//! limits.rs (free_now), precheck_withdraw.rs (held), engine.rs (справочник сетей, виды площадок), settings (accounts).
//! Настройки: venue.quote_asset, venue.bsc_network (читаются, своих нет).

use rust_decimal::Decimal;

use crate::core::types::{CoinNetworkInfo, DexAccountId, VenueId};
use crate::gateway::contract::VenueKind;

use super::engine::{Core, rd};
use super::precheck_withdraw::Held;
use super::usdt_transfer_check::{End, Limits, Quote, check_amount};

/// Вид площадки по реестру ядра. Ошибка: площадки нет.
fn kind(core: &Core, v: &str) -> Result<VenueKind, String> {
    rd(&core.inner.kinds)
        .iter()
        .find(|(id, _)| id.0 == v)
        .map(|(_, k)| *k)
        .ok_or_else(|| format!("площадки {v} нет"))
}

/// Конец перевода из того, что выбрал человек. Принимает: ядро, площадку, кошелёк (у DEX — обязателен).
/// Ошибки: площадки нет; у DEX не выбран кошелёк или его нет на «Маппинге кошельков».
pub fn end(core: &Core, venue: &str, wallet: Option<&str>) -> Result<End, String> {
    let wallet = match kind(core, venue)? {
        VenueKind::Cex => None,
        _ => {
            let w = wallet.ok_or("выберите кошелёк DEX")?;
            core.inner
                .deps
                .settings
                .accounts()
                .wallet(w)
                .ok_or_else(|| format!("кошелька {w} нет"))?;
            Some(DexAccountId(w.to_string()))
        }
    };
    Ok(End {
        venue: VenueId(venue.to_string()),
        wallet,
    })
}

/// Кошелёк DEX перевода (тот конец, что DEX). Ошибки: оба конца — кошельки, или конец один и тот же.
pub fn account(from: &End, to: &End) -> Result<Option<DexAccountId>, String> {
    if from == to {
        return Err("откуда и куда — одно и то же".into());
    }
    match (&from.wallet, &to.wallet) {
        (Some(_), Some(_)) => Err("кошелёк DEX → кошелёк DEX: такого перевода в боте нет — только биржа ↔ кошелёк и биржа ↔ биржа".into()),
        (w @ Some(_), None) | (None, w @ Some(_)) => Ok(w.clone()),
        (None, None) => Ok(None),
    }
}

/// Сеть USDT площадки в справочнике сетей. Принимает: ядро, площадку, тикер, сеть. Ошибок нет: нет данных — None.
fn info(core: &Core, v: &VenueId, ticker: &str, net: &str) -> Option<CoinNetworkInfo> {
    let nets = rd(&core.inner.networks);
    nets.get(v)?
        .iter()
        .find(|i| i.asset.ticker == ticker && i.network == net)
        .cloned()
}

/// Сводка и проверка перевода. Принимает: ядро, концы, сумму (None — «Всё»: всё, что можно вывести).
/// Возвращает: Quote. Ошибки: текст причины — нет данных сети, вывод или депозит закрыт, меньше минимума, больше
/// доступного, дойдёт ноль, сбой баланса или удержаний.
pub async fn quote(
    core: &Core,
    from: &End,
    to: &End,
    amount: Option<Decimal>,
) -> Result<Quote, String> {
    let acc = account(from, to)?;
    let asset = super::usdt_return::usdt(core, &from.venue)?;
    let t = asset.ticker.clone();
    let network = super::transfers::network(core, &asset, &from.venue)?;
    let cex_from = from.wallet.is_none();
    let fi = info(core, &from.venue, &t, &network);
    let (fee, wmin) = match (&fi, cex_from) {
        (_, false) => (None, Decimal::ZERO),
        (None, true) => {
            return Err(format!(
                "{}: нет данных сети вывода {t} ({network}) — не переводим вслепую",
                from.venue.0
            ));
        }
        (Some(i), true) if !i.withdraw_enabled => {
            return Err(format!(
                "{}: вывод {t} в сети {network} закрыт",
                from.venue.0
            ));
        }
        (Some(i), true) => {
            let fee = i
                .withdraw_fee
                .ok_or_else(|| format!("{}: комиссия вывода {t} неизвестна", from.venue.0))?;
            (Some(fee), i.withdraw_min.unwrap_or_default())
        }
    };
    let dmin = if to.wallet.is_none() {
        let net_to = super::transfers::network(core, &asset, &to.venue)?;
        let ti = info(core, &to.venue, &t, &net_to).ok_or_else(|| {
            format!(
                "{}: нет данных депозита {t} ({net_to}) — не переводим вслепую",
                to.venue.0
            )
        })?;
        if !ti.deposit_enabled {
            return Err(format!(
                "{}: депозит {t} в сети {net_to} закрыт",
                to.venue.0
            ));
        }
        ti.deposit_min.unwrap_or_default()
    } else {
        Decimal::ZERO
    };
    let available = super::limits::free_now(core, &from.venue, acc.as_ref(), &t).await?;
    let (withdrawable, note) = if cex_from {
        match super::precheck_withdraw::held(core, &from.venue, &t).await? {
            Held::Nothing => (available, None),
            Held::Unknown(why) => (
                available,
                Some(format!("биржа не сообщает удержаний вывода ({why})")),
            ),
            Held::Usdt { usdt, why } => ((available - usdt).max(Decimal::ZERO), Some(why)),
        }
    } else {
        (available, None)
    };
    let amount =
        super::costs_withdraw::floor_precision(amount.unwrap_or(withdrawable), fi.as_ref());
    // Решение 199: процентная часть комиссии — от суммы этого перевода (фиксированная проверена выше).
    let fee = fee.and(fi.as_ref().and_then(|i| i.fee_for(amount)));
    let lim = Limits {
        withdrawable,
        min: wmin,
        fee: fee.unwrap_or_default(),
        deposit_min: dmin,
    };
    let arrives = check_amount(amount, &lim, &t, (&from.venue.0, &to.venue.0))?;
    Ok(Quote {
        from: from.clone(),
        to: to.clone(),
        ticker: t,
        network,
        available,
        withdrawable,
        amount,
        fee,
        min: wmin,
        arrives,
        note,
    })
}
