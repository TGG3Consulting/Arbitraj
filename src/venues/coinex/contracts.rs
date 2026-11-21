//! Файл: venues/coinex/contracts.rs
//! Что делает: адреса контрактов монет в сети BSC у CoinEx — из справочника монет GET /v2/assets/info
//! (chain_info[n].identity у цепи с именем venue.bsc_contract_chain). В справочнике ввода/вывода
//! (deposit-withdraw-config) поля адреса контракта нет, поэтому адрес берётся отсюда; без адреса монета с CoinEx не
//! сопоставляется с DEX и не торгуется (жёсткое правило CLAUDE.md: адрес у биржи = адрес токена в пуле).
//! Кто вызывает: wallet.rs (весь справочник и одна монета), tests/coinex_real_data.rs.
//! От чего зависит: num.rs.
//! Настройки: нет (имя цепи передаёт wallet.rs — venue.bsc_contract_chain [coinex]).
//!
//! Официально — Get Coin/Token Information GET /assets/info?ccy= («Returns data of all currencies by default»)
//! https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-assets-info: short_name «Coin/token ticker»,
//! chain_info[n].chain_name «Chain name», chain_info[n].identity — «The unique identifier for the public chain coins,
//! whereas in most chains» — адрес контракта (пример: ERC20 CET — 0x081f…d97f; нативная CSC — пусто).
//! РИСК (анализ docs/notes/coinex-analysis-2026-11-05.md, п. 2): имена цепей здесь и в deposit-withdraw-config, судя по
//! примерам, РАЗНЫЕ («Ethereum (ERC20)» против «CSC»/chain) — как сопоставить, документация не говорит. Поэтому имя цепи
//! BSC для этого справочника — отдельная настройка venue.bsc_contract_chain (задаёт Тигран по живому ответу), не
//! выводится из venue.bsc_network. Адрес из explorer_asset_url (ссылка обозревателя) НЕ разбирается — это догадка.
//! Подпись запроса на странице не указана — wallet.rs шлёт подписанным (проверить на живом ответе).

use std::collections::HashMap;

use alloy_primitives::Address;
use serde_json::Value;

use crate::gateway::errors::GatewayResult;

use super::num;

/// Итог разбора /assets/info.
#[derive(Default)]
pub struct Contracts {
    /// Тикер → адрес контракта в цепи BSC (identity непустой и разобран как адрес 0x…).
    pub by_ticker: HashMap<String, Address>,
    /// Монеты с цепью BSC, у которых identity пуст или не адрес: «тикер: identity» (для журнала).
    pub bad: Vec<String>,
}

/// Что делает: адреса BSC из data /assets/info (массив монет).
/// Принимает: data, имя цепи BSC в этом справочнике (chain_name). Возвращает: Contracts.
/// Ошибка BadResponse — data не массив.
pub fn parse(data: &Value, bsc_chain: &str) -> GatewayResult<Contracts> {
    let coins = data
        .as_array()
        .ok_or_else(|| super::errors::bad("assets/info: data не массив", ""))?;
    let mut out = Contracts::default();
    for c in coins {
        let Some(ticker) = num::text_opt(c, "short_name") else {
            continue;
        };
        let Some(chains) = c.get("chain_info").and_then(Value::as_array) else {
            continue;
        };
        for n in chains {
            if num::text_opt(n, "chain_name").as_deref() != Some(bsc_chain) {
                continue;
            }
            let id = num::text_opt(n, "identity").unwrap_or_default();
            match id.trim().parse::<Address>() {
                Ok(a) => {
                    out.by_ticker.insert(ticker.clone(), a);
                }
                Err(_) => out.bad.push(format!("{ticker}: «{id}»")),
            }
        }
    }
    Ok(out)
}
