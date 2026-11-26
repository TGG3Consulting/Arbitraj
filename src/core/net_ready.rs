//! Файл: core/net_ready.rs
//! Что делает: решение 489 — ручное действие или сделка, которой нужна сеть, ещё не перечитанная после её захвата
//! (net_hold.rs: «Перегнать» по «не вернулись», «Вернуть боту» на выключенной площадке DEX; толчок перечитывания —
//! net_kick.rs), не делает неверного: деньги не уходят без адреса USDT сети и без точности / шага вывода — понятная
//! причина «сеть … перечитывается — повторите». «Перегнать» с такой причиной возвращает часть в «не вернулись»
//! (manual_usdt.rs, одна попытка), возврат после закрытия — в повторы (usdt_return_retry.rs). Монеты сети по умолчанию
//! (BSC) — как раньше: проверок нет.
//! Кто вызывает: core/usdt_return.rs (usdt — адрес USDT сети площадки DEX), core/transfers.rs (go — перед выводом).
//! От чего зависит: registry_chains.rs (usdt_of), dirctx_venue.rs (запись справочника сетей), daily_venue.rs,
//! engine.rs (Inner.net_held).
//! Настройки: нет.

use alloy_primitives::Address;

use crate::core::types::{Asset, ChainId, VenueId};

use super::engine::{Core, lk};

/// Причина «сеть перечитывается» (общая для USDT и вывода). Ошибок нет.
fn rereading(c: &ChainId, v: &VenueId) -> String {
    format!(
        "сеть {c}: справочник сетей бирж перечитывается после захвата сети площадкой {} (решение 489) — повторите",
        v.0
    )
}

/// Что делает: адрес USDT сети площадки (registry_chains::usdt_of); сеть захвачена площадкой, а справочник ещё не
/// перечитан, — причина «перечитывается». Принимает: ядро, площадку, сеть (канонический вид). Возвращает: адрес.
/// Ошибки: текст — адреса нет (причина usdt_of или «перечитывается»).
pub fn usdt_of(core: &Core, v: &VenueId, tag: Option<&ChainId>) -> Result<Address, String> {
    super::registry_chains::usdt_of(core, tag).map_err(|e| {
        let held = lk(&core.inner.net_held).contains(v);
        match tag {
            Some(c) if held => format!("{}; {e}", rereading(c, v)),
            _ => e,
        }
    })
}

/// Что делает: вывод монеты другой сети с биржи — у неё есть запись справочника сетей этой сети (точность и шаг вывода,
/// dirctx_venue::network); нет — вывода нет. Монета сети по умолчанию и вывод с DEX — без проверки (как раньше).
/// Принимает: ядро, отправителя, монету, сеть вывода. Ошибки: текст — записи нет (сеть перечитывается).
pub fn withdraw(core: &Core, from: &VenueId, asset: &Asset, net: &str) -> Result<(), String> {
    let Some(c) = asset.chain.as_ref() else {
        return Ok(());
    };
    if super::daily_venue::is_dex(core, &from.0) {
        return Ok(());
    }
    match super::dirctx_venue::network(core, from, asset, net) {
        Some(_) => Ok(()),
        None => Err(format!(
            "{}: записи сети {net} для {} в справочнике сетей нет — точность и шаг вывода неизвестны, вывода нет; {}",
            from.0,
            asset.ticker,
            rereading(c, from)
        )),
    }
}

/// Для прямого возврата в дополнительной сети собственный каталог стороны CEX должен
/// подтверждать контракт валюты учёта и выбранную сторону операции. Адрес другой биржи
/// подтверждает существование токена в сети, но не приём или вывод у этого участника.
pub(super) fn direct_cash(
    core: &Core,
    cex: &VenueId,
    asset: &Asset,
    withdraw: bool,
) -> Result<(), String> {
    let Some(chain) = asset.chain.as_ref() else {
        return Ok(());
    };
    let at = crate::settings::read::At {
        venue: Some(&cex.0),
        coin: None,
    };
    let network = crate::gateway::own_nets::name_of(&core.inner.deps.settings, at, Some(chain))
        .map_err(|e| e.message)?;
    let all = super::engine::rd(&core.inner.networks);
    let rows = all.get(cex).map(|v| v.as_slice()).unwrap_or_default();
    let row = super::settlement_network::unique(rows, cex, &network, asset)?;
    if row.asset.ticker != asset.ticker
        || (withdraw && !row.withdraw_enabled)
        || (!withdraw && !row.deposit_enabled)
    {
        return Err(format!(
            "{}: {} {} в сети {} не подтверждён",
            cex.0,
            if withdraw {
                "вывод"
            } else {
                "депозит"
            },
            asset.ticker,
            network
        ));
    }
    Ok(())
}
