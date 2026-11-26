//! Файл: venues/bsc_dex/tokens.rs
//! Что делает: токены, с которыми работает адаптер: монеты из подписки ядра (тикер + адрес контракта из API
//! биржи) и их decimals, прочитанные из самих контрактов (ERC-20 decimals()) один раз.
//! Кто вызывает: mod.rs (subscribe_market кладёт монеты), wallet/balances.rs, pools/*, quote/*, exec/*.
//! От чего зависит: ctx.rs, node/call.rs, node/abi.rs, core/types (Asset).
//! Настройки: venue.quote_asset [bsc_dex] (тикер USDT).
//!
//! decimals — из контракта токена: https://eips.ethereum.org/EIPS/eip-20 (decimals). Адрес токена — только из
//! справочника ядра (API биржи, CLAUDE.md: «адрес контракта у биржи совпадает с адресом токена в пуле»).
//! Решение 239: pinned — копия списка и decimals из памяти на момент снимка котировки (venues/bsc_dex/frozen.rs):
//! котировка по снимку узел не спрашивает; decimals, которых в памяти не было, — ошибка StaleData, живые не читаются.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use alloy_primitives::Address;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::Ctx;
use super::node::abi::IERC20;
use super::node::call;

/// Известный токен.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub ticker: String,
    pub address: Address,
}

#[derive(Default)]
struct Subscription {
    tokens: Vec<Token>,
    /// Успешная привязка текущей подписки и поколения настроек. Обновление подписки стирает её под тем же замком.
    quote: Option<(&'static str, u64, Address)>,
}

/// Список токенов и кэш decimals. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Tokens {
    list: Arc<RwLock<Subscription>>,
    decimals: Arc<RwLock<HashMap<Address, u8>>>,
    /// Решение 239: копия на момент снимка — decimals только из памяти, без узла.
    pinned: bool,
}

impl Tokens {
    /// Заменяет список токенов монетами из подписки ядра; монеты без адреса в BSC пропускаются. Решение 211: монета —
    /// адрес контракта, тикер — её показное имя; один адрес — одна запись (первая), даже если ядро прислало его дважды.
    /// Принимает: монеты. Ошибок нет.
    pub fn set(&self, assets: &[Asset]) {
        let mut seen = std::collections::HashSet::new();
        let list = assets
            .iter()
            .filter_map(|a| {
                a.contract.map(|address| Token {
                    ticker: a.ticker.clone(),
                    address,
                })
            })
            .filter(|t| seen.insert(t.address))
            .collect();
        *self.list.write().unwrap_or_else(|p| p.into_inner()) = Subscription {
            tokens: list,
            quote: None,
        };
    }

    /// Текущий список токенов. Ошибок нет.
    pub fn all(&self) -> Vec<Token> {
        self.list
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .tokens
            .clone()
    }

    /// Тикер токена по адресу. None — такой монеты в подписке нет.
    /// Решение 403: поиск под замком чтения, копируется только найденное (без копии всего списка).
    pub fn ticker_of(&self, address: Address) -> Option<String> {
        self.list
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .tokens
            .iter()
            .find(|t| t.address == address)
            .map(|t| t.ticker.clone())
    }

    /// Адрес USDT — монеты с тикером venue.quote_asset [bsc_dex] из подписки ядра (адрес — из API биржи).
    /// Ошибки: NotConfigured — тикер не задан или такой монеты с адресом в подписке нет.
    pub fn usdt(&self, ctx: &Ctx) -> GatewayResult<Address> {
        let generation = ctx.settings.generation();
        let venue = ctx.place.id;
        if let Some((place, revision, addr)) =
            self.list.read().unwrap_or_else(|p| p.into_inner()).quote
            && place == venue
            && revision == generation
        {
            return Ok(addr);
        }
        let quote = ctx.text("venue.quote_asset")?;
        let chain = ctx.text("dex.chain").ok();
        let pinned = crate::settings::evm_profiles::settlement_contract(
            &ctx.settings,
            venue,
            chain.as_deref().unwrap_or(""),
            &quote,
        )
        .map_err(|why| ctx.not_set("dex.settlement_contract", why))?;
        let mut subscription = self.list.write().unwrap_or_else(|p| p.into_inner());
        let found = selected_quote(&subscription.tokens, &quote, pinned).ok_or_else(|| {
            let contract = pinned
                .map(|a| format!(" с контрактом {a:#x}"))
                .unwrap_or_default();
            ctx.not_set(
                "venue.quote_asset",
                format!("В списке подписки нет {quote}{contract}"),
            )
        })?;
        subscription.quote = Some((venue, generation, found));
        Ok(found)
    }

    /// Токен по тикеру. None — такой монеты в подписке нет.
    /// Решение 403: поиск под замком чтения, копируется только найденный токен (раньше — весь список на каждый вызов,
    /// а usdt() зовётся на каждой котировке).
    pub fn by_ticker(&self, ticker: &str) -> Option<Token> {
        self.list
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .tokens
            .iter()
            .find(|t| t.ticker == ticker)
            .cloned()
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

    /// Решение 239: копия списка токенов и decimals из памяти на этот момент (снимок котировки, frozen.rs) — своя
    /// память, узел не спрашивается. Возвращает: Tokens. Ошибок нет.
    pub fn pinned(&self) -> Tokens {
        Tokens {
            list: Arc::new(RwLock::new(Subscription {
                tokens: self.all(),
                quote: None,
            })),
            decimals: Arc::new(RwLock::new(
                self.decimals
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            )),
            pinned: true,
        }
    }

    /// decimals токена: из кэша или из контракта (decimals()); у копии на момент снимка (решение 239) — только из
    /// памяти. Принимает: адрес. Возвращает: число знаков. Ошибки: узел, контракт не ERC-20; у копии — StaleData
    /// (decimals на момент снимка в памяти не было).
    pub async fn decimals(&self, ctx: &Ctx, token: Address) -> GatewayResult<u8> {
        if let Some(d) = self
            .decimals
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&token)
        {
            return Ok(*d);
        }
        if self.pinned {
            return Err(ctx.err(
                ErrorCode::StaleData,
                format!("decimals токена {token} на момент снимка в памяти не было — по снимку не посчитать"),
            ));
        }
        let d = call::view(ctx, token, &IERC20::decimalsCall {}, "latest").await?;
        self.decimals
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(token, d);
        Ok(d)
    }
}

/// Выбирает контракт из подписки по настроенному тикеру и подтверждённому адресу.
fn selected_quote(tokens: &[Token], quote: &str, contract: Option<Address>) -> Option<Address> {
    tokens
        .iter()
        .find(|t| t.ticker == quote && contract.is_none_or(|a| a == t.address))
        .map(|t| t.address)
}

#[cfg(test)]
mod settlement_asset_tests {
    use super::*;

    #[test]
    fn bound_quote_requires_the_actual_contract_in_both_subscription_orders() {
        let cash = Token {
            ticker: "USDC".into(),
            address: Address::repeat_byte(1),
        };
        let other = Token {
            ticker: "USDC".into(),
            address: Address::repeat_byte(2),
        };
        for list in [[cash.clone(), other.clone()], [other.clone(), cash.clone()]] {
            assert_eq!(
                selected_quote(&list, "USDC", Some(cash.address)),
                Some(cash.address)
            );
            assert_eq!(selected_quote(&list, "USDC", None), Some(list[0].address));
            assert_eq!(selected_quote(&list, "USDT", Some(cash.address)), None);
        }
        assert_eq!(selected_quote(&[other], "USDC", Some(cash.address)), None);
    }
}
