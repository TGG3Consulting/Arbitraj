//! Файл: core/transfers.rs
//! Что делает: перевод монеты или USDT между площадками по сети BSC (шаг 5 ТЗ): адрес депозита на
//! получателе → вывод с отправителя → хеш транзакции → зачисление. Статусы — через шлюз с частотой
//! venue.status_poll_ms; зачисление — по событию баланса или опросу, кто первый (решение 76: до вывода регистрируется
//! ожидаемое зачисление — credit_expect.rs, credit_watch.rs; шаг зачисления — transfer_credit.rs). Перевод дольше transfer.max_time_s —
//! уведомление и карантин монеты, ждём дальше (монеты в пути). Фактическое время в пути пишется в историю
//! (для T в расчёте, dirctx.rs). Сумма вывода — вниз до точности вывода биржи (решение 104).
//! Решение 151: перевод сделки на кошелёк DEX (send_trade) засчитывается и по приходу монеты в блокчейне, не дожидаясь
//! хеша от биржи-отправителя (credit_chain.rs, transfer_chain.rs; transfer.credit_by_chain получателя). Перегон
//! (send), возврат USDT (send_as), биржа ↔ биржа и переводы на биржу — как раньше (решение 76).
//! Кто вызывает: executor.rs (перевод сделки — send_trade), relay.rs (перегон), usdt_relay_retry.rs (USDT).
//! От чего зависит: engine.rs, wait.rs, rules_apply.rs, risk.rs, registry (сети BSC площадок), credit_expect.rs,
//! credit_watch.rs, transfer_credit.rs, credit_chain.rs, transfer_chain.rs.
//! Недостача зачисления больше transfer.credit_shortfall_pct — монета в ручной чёрный список (credit_shortfall.rs).
//! Настройки: venue.status_poll_ms (по площадке), transfer.max_time_s (по площадке получателя и монете), venue.bsc_network.

use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, Transfer, TransferStatus, VenueId, WithdrawRequest};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::engine::{Core, rd};
use super::op_error::OpError;
use super::rules_apply::{self, Next};
use super::wait::{Pacer, Unit};

/// Название сети BSC площадки для монеты (у биржи — из её справочника; у DEX — своя venue.bsc_network, затем сеть
/// любой биржи монеты).
/// Ошибка: ни у одной биржи монеты нет сети BSC.
pub fn network(core: &Core, asset: &Asset, venue: &VenueId) -> Result<String, String> {
    let coins = rd(&core.inner.coins);
    // Решение 131: по карте адрес → монеты, без прохода справочника.
    let entry = asset
        .bsc_contract
        .and_then(|t| super::ticker_index::find_by_token(core, &coins, t));
    let any = rd(&core.inner.networks)
        .get(venue)
        .and_then(|l| {
            l.iter()
                .find(|i| i.asset.ticker == asset.ticker && i.asset.bsc_contract.is_some())
        })
        .map(|i| i.network.clone());
    let own = entry.and_then(|e| {
        e.presence
            .iter()
            .find(|p| &p.venue == venue)
            .and_then(|p| p.network.clone())
    });
    // Решение 108: у каждой площадки своё название сети BSC (BingX и DEX — BEP20, MEXC — BSC). Своё — раньше
    // чужого: иначе DEX получал от MEXC «BSC» и отказывал («Кошелёк DEX работает только в сети BEP20»).
    // Решение 118 (Я-19): только своё название площадки — название сети другой площадки не годится (BingX «BEP20»,
    // MEXC «BSC»). Нет своего — ошибка: задайте venue.bsc_network площадки.
    own.or(any)
        .or_else(|| own_bsc_name(core, venue))
        .ok_or_else(|| {
            format!(
                "{}: нет сети BSC для {} — задайте venue.bsc_network площадки",
                venue.0, asset.ticker
            )
        })
}

/// Название сети BSC самой площадки — настройка venue.bsc_network (решение 108). None — не задана. Ошибок нет.
pub fn own_bsc_name(core: &Core, venue: &VenueId) -> Option<String> {
    core.inner
        .deps
        .settings
        .text(
            "venue.bsc_network",
            crate::settings::read::At {
                venue: Some(&venue.0),
                coin: None,
            },
        )
        .ok()
}

/// Переводит qty монеты с from на to. Возвращает: (вывод с хешем и комиссией, зачисление — сколько дошло).
/// Ошибки: текст причины (вывод отклонён, правило шлюза остановило) — уже в журнале и уведомлениях.
pub async fn send(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    qty: Decimal,
) -> Result<(Transfer, Transfer), OpError> {
    // Решение 132: DEX — кошелёк сделки (адрес депозита на DEX, вывод с кошелька — его).
    let acc = super::dex_account::of_deal(core, deal)?;
    send_as(core, deal, from, to, asset, qty, acc.as_ref()).await
}

/// Перевод сделки (executor.rs): как send, но на кошелёк DEX сделки зачисляется и по приходу в блокчейне (решение
/// 151). Возвращает: (вывод, зачисление, строка хронологии сделки — если зачислено по блокчейну). Ошибки: как у send.
pub async fn send_trade(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    qty: Decimal,
) -> Result<(Transfer, Transfer, Option<String>), OpError> {
    let acc = super::dex_account::of_deal(core, deal)?;
    go(core, deal, from, to, asset, qty, acc.as_ref(), true).await
}

/// Как send, но счёт DEX задан явно (перегон USDT вне сделки — строка маппинга биржи, решение 132).
/// Принимает: …, счёт DEX (None — DEX не участвует). Ошибки: как у send.
pub async fn send_as(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    qty: Decimal,
    acc: Option<&crate::core::types::DexAccountId>,
) -> Result<(Transfer, Transfer), OpError> {
    let (sent, credited, _) = go(core, deal, from, to, asset, qty, acc, false).await?;
    Ok((sent, credited))
}

/// Перевод. Принимает: как send_as, плюс можно ли зачислить по блокчейну (решение 151 — только перевод сделки).
/// Возвращает: (вывод, зачисление, строка хронологии — если зачислено по блокчейну). Ошибки: как у send.
#[allow(clippy::too_many_arguments)]
async fn go(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    qty: Decimal,
    acc: Option<&crate::core::types::DexAccountId>,
    chain: bool,
) -> Result<(Transfer, Transfer, Option<String>), OpError> {
    let deps = &core.inner.deps;
    let gw_from = deps.gw.get_as(from, acc).await.map_err(|e| e.message)?;
    let gw_to = deps.gw.get_as(to, acc).await.map_err(|e| e.message)?;
    let net_from = network(core, asset, from)?;
    let net_to = network(core, asset, to)?;
    let address = gw_to
        .deposit_address(asset, &net_to)
        .await
        .map_err(|e| format!("{}: адрес депозита: {}", to.0, e.message))?;
    // Ожидаемое зачисление — до вывода, чтобы событие не пришло раньше (решение 76; события у получателя выключены или
    // приход не посчитан — только опрос).
    // Решение 104: сумма вывода — вниз до точности вывода биржи (withdrawPrecision из справочника сетей; так же считает
    // расчёт, costs_withdraw::floor_precision). Без этого хвост вида 14.425559999999999999 (комиссия покупки в монете
    // пришла числом с плавающей точкой) — отказ BingX 100400 «decimal precision … Precision: 8».
    let qty = {
        let nets = rd(&core.inner.networks);
        let info = nets.get(from).and_then(|l| {
            l.iter()
                .find(|i| i.asset.ticker == asset.ticker && i.network == net_from)
        });
        super::costs_withdraw::floor_precision(qty, info)
    };
    if qty <= Decimal::ZERO {
        return Err(OpError::from(format!(
            "{}: сумма вывода {} после округления до точности вывода — ноль",
            from.0, asset.ticker
        )));
    }
    let ticket = super::credit_expect::ticket(core, deal, from, to, asset, &net_from, qty).await;
    if let Some(t) = &ticket {
        t.set_account(acc.cloned());
    }
    let req = WithdrawRequest {
        venue: from.clone(),
        asset: asset.clone(),
        network: net_from.clone(),
        address,
        amount: qty,
        deal_id: deal.to_string(),
    };
    let started = Utc::now();
    // Решение 151: только получатель — DEX и известен кошелёк сделки (credit_chain.rs).
    if chain {
        super::credit_chain::arm(core, ticket.as_ref(), to, acc.is_some(), started);
    }
    let mut attempt = 0;
    let created = loop {
        attempt += 1;
        match gw_from.withdraw(req.clone()).await {
            Ok(t) => break t,
            Err(e) => match rules_apply::on_error(core, "Вывод", from, Some(deal), &e, attempt)
                .await
            {
                Next::Retry { pause_ms } => {
                    tokio::time::sleep(Duration::from_millis(pause_ms)).await
                }
                Next::Stop => {
                    // Вывода не будет — получатель снимает ожидание поступления (DEX: wallet/inflows.rs). Исход
                    // неизвестен — перевод мог уйти: ожидание остаётся (до ручной проверки или срока).
                    if e.code != ErrorCode::OutcomeUnknown {
                        let _ = gw_to.deposit_abandoned(asset).await;
                    }
                    return Err(OpError::from(&e));
                }
            },
        }
    };
    let quiet = Notify::new();
    // 1. Ждём хеш транзакции вывода.
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(from.0.clone()));
    let mut last = String::new();
    let sent = loop {
        // Решение 151: приход в блокчейне уже засчитан зачислением — к продаже, хеш биржи сверяется фоном.
        if let Some(hit) = ticket
            .as_ref()
            .and_then(super::credit_watch::Ticket::chain_hit)
        {
            let c = super::transfer_chain::Chain {
                deal,
                from,
                to,
                asset,
                net_from: &net_from,
                net_to: net_to.clone(),
                qty,
                started,
                created: created.clone(),
                gw_from: gw_from.clone(),
                gw_to: gw_to.clone(),
            };
            let (sent, credited, note) = super::transfer_chain::finish(core, c, hit, ticket).await;
            return Ok((sent, credited, Some(note)));
        }
        // Событие зачисления только будит опрос: хеш вывода — только от отправителя (решение 76).
        match gw_from.withdrawal_status(&created.venue_ref).await {
            Ok(t) if t.tx_hash.is_some() && t.status != TransferStatus::Pending => break t,
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::WithdrawRejected => {
                rules_apply::on_error(core, "Вывод", from, Some(deal), &e, 1).await;
                let _ = gw_to.deposit_abandoned(asset).await;
                return Err(OpError::from(&e));
            }
            Err(e) if e.message != last => {
                deps.log(
                    Level::Warn,
                    "core.transfer",
                    Some(&from.0),
                    format!("Статус вывода {}: {}", created.venue_ref, e.message),
                    None,
                )
                .await;
                last = e.message;
            }
            Err(_) => {}
        }
        super::transfer_watch::slow(core, deal, to, asset, started).await;
        let kick = ticket.as_ref().map(|t| t.wake.as_ref()).unwrap_or(&quiet);
        pacer.pause(deps, kick).await;
    };
    let tx = sent.tx_hash.clone().unwrap_or_default();
    // Хеш отправителя известен: события с другим хешем — не этого перевода, это внешние изменения баланса.
    if let Some(t) = &ticket {
        for b in t.set_tx(&tx) {
            super::credit_event::external(core, &b).await;
        }
    }
    let text = format!(
        "{} {qty} {}: {} → {}, хеш {tx}",
        deal, asset.ticker, from.0, to.0
    );
    deps.log(
        Level::Info,
        "core.transfer_sent",
        Some(&from.0),
        &text,
        None,
    )
    .await;
    let tg = crate::notify::human::Msg::new(crate::notify::human::INFO, "Перевод отправлен")
        .line(format!(
            "{} {} {}",
            crate::notify::human::qty(qty),
            asset.ticker,
            crate::notify::human::route(&from.0, &to.0)
        ))
        .kv("Транзакция", crate::notify::human::addr(&tx))
        .id(deal)
        .render();
    deps.notify("deal.transfer_sent", Some(&from.0), &tg).await;
    // 2. Ждём зачисления на получателе: событие или опрос (transfer_credit.rs).
    let leg = super::transfer_credit::Leg {
        deal,
        from,
        to,
        asset,
        network: net_to,
        tx,
        started,
    };
    let credited = super::transfer_credit::wait(core, &leg, gw_to, ticket).await;
    // Решение 118 (Я-3): дошло меньше ожидаемого больше допуска — монета в ручной чёрный список.
    let (f, a) = (&net_from, credited.amount);
    super::credit_shortfall::check(core, deal, from, to, asset, f, qty, a).await;
    Ok((sent, credited, None))
}
