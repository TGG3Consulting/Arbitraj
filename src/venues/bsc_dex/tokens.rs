//! Файл: venues/bsc_dex/tokens.rs
//! Что делает: токены, с которыми работает адаптер: монеты из подписки ядра (тикер + адрес контракта из API
//! биржи) и их decimals, прочитанные из самих контрактов (ERC-20 decimals()) один раз.
//! Кто вызывает: mod.rs (subscribe_market кладёт монеты), wallet/balances.rs, pools/*, quote/*, exec/*.
//! От чего зависит: ctx.rs, node/call.rs, node/abi.rs, core/types (Asset).
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT).
//!
//! decimals — из контракта токена: https://eips.ethereum.org/EIPS/eip-20 (decimals). Адрес токена — только из
//! справочника ядра (API биржи, CLAUDE.md: «адрес контракта у биржи совпадает с адресом токена в пуле»).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::Asset;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::node::abi::IERC20;
use super::node::call;

/// Известный токен.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub ticker: String,
    pub address: Address,
}

/// Список токенов и кэш decimals. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Tokens {
    list: Arc<RwLock<Vec<Token>>>,
    decimals: Arc<RwLock<HashMap<Address, u8>>>,
}

impl Tokens {
    /// Заменяет список токенов монетами из подписки ядра; монеты без адреса в BSC пропускаются.
    /// Принимает: монеты. Ошибок нет.
    pub fn set(&self, assets: &[Asset]) {
        let list = assets
            .iter()
            .filter_map(|a| {
                a.bsc_contract.map(|address| Token {
                    ticker: a.ticker.clone(),
                    address,
                })
            })
            .collect();
        *self.list.write().unwrap_or_else(|p| p.into_inner()) = list;
    }

    /// Текущий список токенов. Ошибок нет.
    pub fn all(&self) -> Vec<Token> {
        self.list.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Тикер токена по адресу. None — такой монеты в подписке нет.
    pub fn ticker_of(&self, address: Address) -> Option<String> {
        self.all()
            .into_iter()
            .find(|t| t.address == address)
            .map(|t| t.ticker)
    }

    /// Адрес USDT — монеты с тикером venue.quote_asset [bsc_dex] из подписки ядра (адрес — из API биржи).
    /// Ошибки: NotConfigured — тикер не задан или такой монеты с адресом в подписке нет.
    pub fn usdt(&self, ctx: &Ctx) -> GatewayResult<Address> {
        let quote = ctx.text("venue.quote_asset")?;
        self.by_ticker(&quote).map(|t| t.address).ok_or_else(|| {
            ctx.not_set(
                "venue.quote_asset",
                format!("В списке монет ядра нет {quote} (venue.quote_asset) с адресом в BSC"),
            )
        })
    }

    /// Токен по тикеру. None — такой монеты в подписке нет.
    pub fn by_ticker(&self, ticker: &str) -> Option<Token> {
        self.all().into_iter().find(|t| t.ticker == ticker)
    }

    /// Запоминает decimals токена, уже прочитанные из контракта другим путём (ответ decimals() из записи узла —
    /// тесты на записанных данных). Ошибок нет.
    pub fn remember_decimals(&self, token: Address, d: u8) {
        self.decimals
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, d);
    }

    /// decimals токена, если уже известны (из кэша, без узла). Ошибок нет.
    pub fn decimals_known(&self, token: Address) -> Option<u8> {
        self.decimals
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&token)
            .copied()
    }

    /// decimals токена: из кэша или из контракта (decimals()).
    /// Принимает: адрес. Возвращает: число знаков. Ошибки: узел, контракт не ERC-20.
    pub async fn decimals(&self, ctx: &Ctx, token: Address) -> GatewayResult<u8> {
        if let Some(d) = self
            .decimals
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&token)
        {
            return Ok(*d);
        }
        let d = call::view(ctx, token, &IERC20::decimalsCall {}, "latest").await?;
        self.decimals
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, d);
        Ok(d)
    }
}
