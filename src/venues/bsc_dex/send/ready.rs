//! Файл: venues/bsc_dex/send/ready.rs
//! Что делает: готовность канала к отправке по его настройкам и ключу (решения 41, 43, 44) и сама отправка
//! одной транзакции или бандла в формате канала; разбор ответа (принято / ошибка канала как есть).
//! Канал с ключом без введённого ключа не используется (ChannelState::NoKey).
//! Кто вызывает: send/mod.rs, send/check.rs.
//! От чего зависит: send/channel.rs, send/sequencer_cond.rs, node/rpc.rs (POST), node/jsonrpc.rs,
//! node/arb_min_gas.rs, errors.rs, ctx.rs.
//! Настройки: channel.enabled, channel.url, channel.method, channel.min_gas_price_gwei, channel.tip_enabled,
//! channel.tip_bnb, channel.tip_address, channel.cond_time_window_s [канал]; dex.arb_gas_info [площадка DEX]; секрет channel.<id>.key (открытая отправка — bsc.node_http_key).
//!
//! Решение 436: у секвенсора Arbitrum (Single::Sequencer) минимум цены газа — с узла (node/arb_min_gas.rs floor), окно
//! условной отправки — channel.cond_time_window_s (send/sequencer_cond.rs); всё читается здесь, до отправки (решение 398).
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
use super::super::node::{arb_min_gas, jsonrpc, rpc};
use super::super::num;
use super::bloxroute_cloud::NETWORK;
use super::channel::{Auth, Bundle, Channel, Single};
use super::sequencer_cond::{self, Cond};

/// Канал, готовый к отправке.
pub struct Ready {
    pub ch: &'static Channel,
    url: Zeroizing<String>,
    key: Option<Zeroizing<String>>,
    pub method: SendMethod,
    /// Минимальная цена газа канала, wei (секвенсор Arbitrum — с узла, решение 436).
    pub min_gas_wei: U256,
    /// Решение 436: окно условной отправки, секунды (channel.cond_time_window_s; только Single::Sequencer). None —
    /// обычная отправка.
    pub cond: Option<u64>,
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
    let min_gas_wei = min_gas(ctx, ch, &s)?;
    let cond = match ch.single {
        Some(Single::Sequencer) => sequencer_cond::window(ctx, &s).map_err(nc)?,
        _ => None,
    };
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
        cond,
        tip,
    })
}

/// Минимальная цена газа канала, wei: решение 44 — channel.min_gas_price_gwei; решение 436 — у секвенсора Arbitrum
/// с узла (node/arb_min_gas.rs floor). Принимает: окружение, канал, его область. Ошибки: состояние NotConfigured с
/// причиной (не задана, не в wei, минимум с узла не прочитан).
fn min_gas(ctx: &Ctx, ch: &Channel, s: &Scope) -> Result<U256, ChannelState> {
    if ch.min_from_node() {
        return arb_min_gas::floor(ctx).map_err(|reason| ChannelState::NotConfigured { reason });
    }
    let nc = |e: GatewayError| ChannelState::NotConfigured { reason: e.message };
    let gwei = ctx.settings.decimal_in("channel.min_gas_price_gwei", s).map_err(nc)?;
    num::to_units(gwei * Decimal::from(1_000_000_000u64), 0).ok_or_else(|| {
        ChannelState::NotConfigured {
            reason: "channel.min_gas_price_gwei не переводится в wei".into(),
        }
    })
}

impl Ready {
    /// Отправка: одиночная — первая транзакция; бандл — все. Принимает: подписанные транзакции, предел блока бандла,
    /// время головы (решение 436: у канала с окном — условие timestampMax = время + окно; None — без условия).
    /// Возвращает: Ok — канал принял. Ошибки: ответ канала (ChannelRejected, GasBelowChannelMin, NonceConflict…), сеть, HTTP.
    pub async fn send(
        &self,
        ctx: &Ctx,
        txs: &[Bytes],
        max_block: u64,
        head_time: Option<u64>,
    ) -> GatewayResult<()> {
        let cond = match (self.cond, head_time) {
            (Some(w), Some(t)) => Cond::Until(t.saturating_add(w)),
            _ => Cond::Plain,
        };
        self.post(ctx, txs, max_block, cond).await
    }

    /// «Проверить подключение» (send/check.rs): пустая транзакция тем методом, которым канал отправляет; у канала с
    /// окном (решение 436) — условный метод с пустыми условиями. Ошибки: как send.
    pub async fn probe(&self, ctx: &Ctx) -> GatewayResult<()> {
        let cond = if self.cond.is_some() {
            Cond::Probe
        } else {
            Cond::Plain
        };
        self.post(ctx, &[Bytes::new()], 0, cond).await
    }

    /// Запрос в канал и разбор ответа. Принимает: транзакции, предел блока, условие. Ошибки: как send; у условной
    /// отправки — с пояснением кода секвенсора (send/sequencer_cond.rs explain).
    async fn post(
        &self,
        ctx: &Ctx,
        txs: &[Bytes],
        max_block: u64,
        cond: Cond,
    ) -> GatewayResult<()> {
        let conditional = self.ch.single == Some(Single::Sequencer) && cond != Cond::Plain;
        let body = self.body(txs, max_block, cond);
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
            Err(e) if conditional => Err(sequencer_cond::explain(&text, e)),
            Err(e) => Err(e),
        }
    }

    /// Тело запроса в формате канала (условие — только у Single::Sequencer).
    fn body(&self, txs: &[Bytes], max_block: u64, cond: Cond) -> Value {
        let no0x = |b: &Bytes| alloy_primitives::hex::encode(b);
        let (method, params) = match (self.method, self.ch.single, self.ch.bundle) {
            (SendMethod::Single, Some(Single::Raw), _) => {
                ("eth_sendRawTransaction", json!([txs[0]]))
            }
            (SendMethod::Single, Some(Single::Sequencer), _) if cond != Cond::Plain => {
                (sequencer_cond::METHOD, sequencer_cond::params(&txs[0], cond))
            }
            (SendMethod::Single, Some(Single::Sequencer), _) => {
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
