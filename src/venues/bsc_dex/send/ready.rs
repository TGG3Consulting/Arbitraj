//! Файл: venues/bsc_dex/send/ready.rs
//! Что делает: готовность канала к отправке по его настройкам и ключу (решения 41, 43, 44) и сама отправка
//! одной транзакции или бандла в формате канала; разбор ответа (принято / ошибка канала как есть).
//! Канал с ключом без введённого ключа не используется (ChannelState::NoKey).
//! Кто вызывает: send/mod.rs, send/check.rs.
//! От чего зависит: send/channel.rs, node/rpc.rs (POST), node/jsonrpc.rs, errors.rs, ctx.rs.
//! Настройки: channel.enabled, channel.url, channel.method, channel.min_gas_price_gwei, channel.tip_enabled,
//! channel.tip_bnb, channel.tip_address [канал]; секрет channel.<id>.key (открытая отправка — bsc.node_http_key).
//!
//! «already known» — транзакция уже принята этим узлом/каналом: не ошибка (research/flow/bsc_send.md, раздел 6 п. 5;
//! текст — core/txpool/errors.go#L26 узла BSC).

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::core::types::{ChannelState, SendMethod};
use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;
use super::super::errors::Op;
use super::super::node::rpc::SECRET_HTTP_KEY;
use super::super::node::{jsonrpc, rpc};
use super::super::num;
use super::bloxroute_cloud::NETWORK;
use super::channel::{Auth, Bundle, Channel, Single};

/// Канал, готовый к отправке.
pub struct Ready {
    pub ch: &'static Channel,
    url: Zeroizing<String>,
    key: Option<Zeroizing<String>>,
    pub method: SendMethod,
    /// Минимальная цена газа канала, wei.
    pub min_gas_wei: U256,
    /// Чаевые: (адрес, сумма в wei); None — выключены.
    pub tip: Option<(Address, U256)>,
}

/// Готов ли канал. Возвращает: Ready или состояние, почему нет (Disabled, NoKey, NotConfigured с причиной).
pub fn prepare(ctx: &Ctx, ch: &'static Channel) -> Result<Ready, ChannelState> {
    let s = Scope::Channel(ch.id.to_string());
    let st = &ctx.settings;
    let nc = |e: GatewayError| ChannelState::NotConfigured { reason: e.message };
    if !st.flag_in("channel.enabled", &s).map_err(nc)? {
        return Err(ChannelState::Disabled);
    }
    let key = match ch.secret() {
        Some(name) => Some(ctx.vault.get(&name).map_err(|_| ChannelState::NoKey)?),
        None => None,
    };
    let raw_url = st.text_in("channel.url", &s).map_err(nc)?;
    let url = match (ch.auth, &key) {
        (Auth::UrlKey, Some(k)) => {
            Zeroizing::new(raw_url.replace(super::super::ctx::KEY_MARK, k.as_str()))
        }
        // Открытая отправка: ключ провайдера узла (секрет bsc.node_http_key) вместо {key}; не введён — NoKey.
        (Auth::NodeKey, _) => ctx
            .with_key(&raw_url, SECRET_HTTP_KEY)
            .map_err(|_| ChannelState::NoKey)?,
        _ => Zeroizing::new(raw_url),
    };
    let method = match st.text_in("channel.method", &s).map_err(nc)?.as_str() {
        "single" if ch.single.is_some() => SendMethod::Single,
        "bundle" if ch.bundle.is_some() => SendMethod::Bundle,
        m => {
            return Err(ChannelState::NotConfigured {
                reason: format!(
                    "channel.method = {m}: у канала {} такого способа нет",
                    ch.title
                ),
            });
        }
    };
    let gwei = st
        .decimal_in("channel.min_gas_price_gwei", &s)
        .map_err(nc)?;
    let min_gas_wei =
        num::to_units(gwei * Decimal::from(1_000_000_000u64), 0).ok_or_else(|| {
            ChannelState::NotConfigured {
                reason: "channel.min_gas_price_gwei не переводится в wei".into(),
            }
        })?;
    let tip = if ch.tips && st.flag_in("channel.tip_enabled", &s).map_err(nc)? {
        let bnb = st.decimal_in("channel.tip_bnb", &s).map_err(nc)?;
        let wei =
            num::to_units(bnb, num::BNB_DECIMALS).ok_or_else(|| ChannelState::NotConfigured {
                reason: "channel.tip_bnb не переводится в wei".into(),
            })?;
        Some((st.address_in("channel.tip_address", &s).map_err(nc)?, wei))
    } else {
        None
    };
    Ok(Ready {
        ch,
        url,
        key,
        method,
        min_gas_wei,
        tip,
    })
}

impl Ready {
    /// Отправка: одиночная — первая транзакция; бандл — все. Принимает: подписанные транзакции, предел блока бандла.
    /// Возвращает: Ok — канал принял. Ошибки: ответ канала (ChannelRejected, GasBelowChannelMin, NonceConflict…), сеть, HTTP.
    pub async fn send(&self, ctx: &Ctx, txs: &[Bytes], max_block: u64) -> GatewayResult<()> {
        let body = self.body(txs, max_block);
        let auth;
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let (Auth::Header, Some(k)) = (self.ch.auth, &self.key) {
            auth = k.clone();
            headers.push(("Authorization", auth.as_str()));
        }
        let text = rpc::post(ctx, &self.url, &body, &headers, self.ch.title).await?;
        match jsonrpc::parse_text(&text, self.ch.title, Op::Send) {
            Ok(_) => Ok(()),
            Err(e) if e.message.to_ascii_lowercase().contains("already known") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Тело запроса в формате канала.
    fn body(&self, txs: &[Bytes], max_block: u64) -> Value {
        let no0x = |b: &Bytes| alloy_primitives::hex::encode(b);
        let (method, params) = match (self.method, self.ch.single, self.ch.bundle) {
            (SendMethod::Single, Some(Single::Raw), _) => {
                ("eth_sendRawTransaction", json!([txs[0]]))
            }
            (SendMethod::Single, Some(Single::Private), _) => {
                ("eth_sendPrivateTransaction", json!([txs[0]]))
            }
            (SendMethod::Single, Some(Single::Bloxroute), _) => {
                ("bsc_private_tx", json!({ "transaction": no0x(&txs[0]) }))
            }
            (_, _, Some(Bundle::Bloxroute)) => (
                "blxr_submit_bundle",
                json!({ "txs": txs.iter().map(no0x).collect::<Vec<_>>(), "blockchain_network": NETWORK, "max_block_number": max_block }),
            ),
            _ => (
                "eth_sendBundle",
                json!([{ "txs": txs, "maxBlockNumber": max_block }]),
            ),
        };
        jsonrpc::request(1, method, &params)
    }
}
