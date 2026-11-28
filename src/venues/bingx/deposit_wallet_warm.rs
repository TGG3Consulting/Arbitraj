//! Файл: venues/bingx/deposit_wallet_warm.rs
//! Что делает: расчётный круг USDC Base — адаптер сам читает адрес депозита расчётного токена в сети Base, чтобы
//! память walletType (deposit_wallet.rs) была заполнена ДО проверки трёх флагов в ядре (core/settlement_model.rs).
//! Ядро читает адрес USDC Base у биржи только в самом переводе круга (core/transfers.rs → deposit_proof.rs), то есть
//! после проверки флагов; предпроверка и прогрев (precheck_deposit.rs, deposit_warm_pass.rs) спрашивают адрес монеты
//! сделки, а не расчётного токена; прогрев у bingx к тому же не задан. Без этого шага флаг зачисления всегда false.
//! Когда: после каждого удачного справочника сетей (Venue::coin_networks — ядро вызывает его само, core/registry_sync.rs:
//! при старте и по venue.networks_poll_ms), в фоне — справочник отдаётся ядру без ожидания. Запрос адреса уходит,
//! только если по этой монете и сети под текущим ключом API walletType ещё не читался (один запрос на ключ; сбой —
//! повтор со следующим справочником). Чтение — тем же deposit_addr.rs (память адреса, tag, walletType), режим «спросить
//! биржу» (gateway/addr_book.rs fresh).
//! Какой токен: монеты справочника BingX, у которых в сети venue.base_network адрес контракта равен одному из
//! сохранённых оператором dex.settlement_contract (адрес расчётного токена DEX, решение «Base USDC» — у base_dex это USDC
//! 0x833589fC…2913). Тикер не зашит: монета находится по адресу контракта.
//! Кто вызывает: mod.rs (Venue::coin_networks).
//! От чего зависит: deposit_wallet.rs, deposit_addr.rs, ctx.rs, settings (Store::all), gateway/addr_book.rs, журнал.
//! Настройки: venue.deposit_to_spot, venue.base_network [bingx]; dex.settlement_contract (все DEX, только чтение).
//! Новых нет.
//!
//! Официальная документация: docs-v3 Account and Wallet → «Main Account Deposit Address»
//! (GET /openApi/wallets/v1/capital/deposit/address, «1/second») — https://bingx-api.github.io/docs-v3/

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alloy_primitives::Address;

use crate::core::types::CoinNetworkInfo;
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID, SECRET_API_KEY};
use super::deposit_addr::{self, DepositAddrs};
use super::deposit_wallet::credit_network;

/// Код записи журнала.
const CODE: &str = "venue.deposit_wallet";

/// Что делает: запускает в фоне чтение адресов расчётного токена Base, которых нет в памяти walletType. Принимает:
/// окружение, память адресов, свежий справочник сетей, флаг «уже идёт» (второй проход не начинается, пока идёт первый).
/// Возвращает: ничего. Ошибок нет: сбой запроса — запись в журнал (Warn), повтор — со следующим справочником.
pub fn spawn(ctx: &Ctx, addrs: &DepositAddrs, list: &[CoinNetworkInfo], busy: &Arc<AtomicBool>) {
    let Some(network) = credit_network(ctx) else {
        return;
    };
    let contracts = settlement_contracts(ctx);
    let mark = ctx.vault.mark(SECRET_API_KEY);
    let todo: Vec<CoinNetworkInfo> = list
        .iter()
        .filter(|n| n.network == network)
        .filter(|n| n.asset.contract.is_some_and(|c| contracts.contains(&c)))
        .filter(|n| !addrs.types.known(mark, &n.asset.ticker, &network))
        .cloned()
        .collect();
    if todo.is_empty() || busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let (ctx, addrs, busy) = (ctx.clone(), addrs.clone(), busy.clone());
    tokio::spawn(async move {
        for n in todo {
            let ask = deposit_addr::deposit_address(&ctx, &addrs, &n.asset, &n.network);
            if let Err(e) = crate::gateway::addr_book::fresh(ask).await {
                let msg = format!(
                    "Адрес депозита {} в сети {} для проверки счёта зачисления (walletType) не получен: {}",
                    n.asset.ticker, n.network, e.message
                );
                let _ = ctx
                    .journal
                    .write(Level::Warn, CODE, Some(ID), msg, None)
                    .await;
            }
        }
        busy.store(false, Ordering::SeqCst);
    });
}

/// Что делает: все сохранённые оператором адреса dex.settlement_contract (любой DEX). Принимает: окружение.
/// Возвращает: список адресов; неразборчивые и нулевые — пропускаются. Ошибок нет.
fn settlement_contracts(ctx: &Ctx) -> Vec<Address> {
    ctx.settings
        .all()
        .into_iter()
        .filter(|(name, scope, _)| {
            name == "dex.settlement_contract" && matches!(scope, Scope::Venue(_))
        })
        .filter_map(|(_, _, v)| v.as_str().and_then(|s| s.parse::<Address>().ok()))
        .filter(|a| !a.is_zero())
        .collect()
}
