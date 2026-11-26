//! Файл: venues/bsc_dex/send/ready.rs
//! Что делает: готовность канала к отправке по его настройкам и ключу (решения 41, 43, 44) и сама отправка
//! одной транзакции или бандла в формате канала; разбор ответа (принято / ошибка канала как есть).
//! Канал с ключом без введённого ключа не используется (ChannelState::NoKey).
//! Кто вызывает: send/mod.rs, send/check.rs.
//! От чего зависит: send/channel.rs, send/sequencer_cond.rs, send/seq_opts.rs, node/rpc.rs (POST), node/jsonrpc.rs,
//! node/arb_min_gas.rs, errors.rs, ctx.rs.
//! Настройки: channel.enabled, channel.url, channel.method, channel.min_gas_price_gwei, channel.tip_enabled,
//! channel.tip_bnb, channel.tip_address, channel.cond_time_window_s, channel.http_timeout_ms, channel.submit_method,
//! channel.sync_timeout_ms [канал]; dex.arb_gas_info [площадка DEX]; секрет channel.<id>.key (открытая отправка —
//! bsc.node_http_key).
//!
//! Решение 436: у секвенсора Arbitrum (Single::Sequencer) минимум цены газа — с узла (node/arb_min_gas.rs floor), окно
//! условной отправки — channel.cond_time_window_s (send/sequencer_cond.rs); всё читается здесь, до отправки (решение 398).
//! Решение 437 (1): у него же метод отправки (channel.submit_method: eth_sendRawTransaction или
//! eth_sendRawTransactionSync) и таймаут HTTP запроса (channel.http_timeout_ms) — send/seq_opts.rs.
//!
//! «already known» — транзакция уже принята этим узлом/каналом: не ошибка (research/flow/bsc_send.md, раздел 6 п. 5;
//! текст — core/txpool/errors.go#L26 узла BSC).

use alloy_primitives::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
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
use super::seq_opts::{self, Submit};
use super::sequencer_cond::{self, Cond, Terms};

/// Канал, готовый к отправке.
pub struct Ready {
    pub ch: &'static Channel,
    endpoint_config: Option<(u64, u64)>,
    url: Zeroizing<String>,
    key: Option<Zeroizing<String>>,
    pub method: SendMethod,
    /// Минимальная цена газа канала, wei (секвенсор Arbitrum — с узла, решение 436).
    pub min_gas_wei: U256,
    /// Решение 436: окно условной отправки, секунды (channel.cond_time_window_s; только Single::Sequencer). None —
    /// обычная отправка.
    pub cond: Option<u64>,
    /// Решение 437: метод отправки секвенсора (channel.submit_method); у остальных каналов — Raw (не читается).
    pub submit: Submit,
    /// Решение 437: таймаут HTTP запроса отправки секвенсора, мс (channel.http_timeout_ms); None — общие http.*.
    pub http_ms: Option<u64>,
    /// Чаевые: (адрес, сумма в wei); None — выключены.
    pub tip: Option<(Address, U256)>,
}

/// Готов ли канал. Возвращает: Ready или состояние, почему нет (Disabled, NoKey, NotConfigured с причиной).
pub fn prepare(ctx: &Ctx, ch: &'static Channel) -> Result<Ready, ChannelState> {
    let endpoint_config = crate::settings::evm_profiles::profile(ctx.place.id).map(|_| {
        (
            ctx.settings.generation(),
            crate::secrets::vault::generation(),
        )
    });
    if !super::channels::compatible(ctx.place.id, ch) {
        return Err(ChannelState::NotConfigured {
            reason: format!("Канал {} не принадлежит площадке {}", ch.id, ctx.place.id),
        });
    }
    let s = Scope::Channel(ch.id.to_string());
    if let Some(p) = crate::settings::evm_profiles::profile(ctx.place.id)
        && !ctx
            .settings
            .text_in("channel.chain", &s)
            .is_ok_and(|chain| chain == p.chain)
    {
        return Err(ChannelState::NotConfigured {
            reason: format!("Задайте channel.chain = {} у {}", p.chain, ch.id),
        });
    }
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
    let (cond, submit, http_ms) = match ch.single {
        Some(Single::Sequencer) => {
            let o = seq_opts::read(ctx, &s).map_err(nc)?;
            (o.cond, o.submit, Some(o.http_ms))
        }
        _ => (None, Submit::Raw, None),
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
        endpoint_config,
        url,
        key,
        method,
        min_gas_wei,
        cond,
        submit,
        http_ms,
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
    let gwei = ctx
        .settings
        .decimal_in("channel.min_gas_price_gwei", s)
        .map_err(nc)?;
    num::to_units(gwei * Decimal::from(1_000_000_000u64), 0).ok_or_else(|| {
        ChannelState::NotConfigured {
            reason: "channel.min_gas_price_gwei не переводится в wei".into(),
        }
    })
}

impl Ready {
    /// Отправка: одиночная — первая транзакция; бандл — все. Принимает: подписанные транзакции, предел блока бандла,
    /// условия (решение 436: у канала с окном — timestampMax = время головы + окно; без времени головы — без условия;
    /// решение 438: вместе с ним knownAccounts пулов маршрута).
    /// Возвращает: Ok — канал принял. Ошибки: ответ канала (ChannelRejected, GasBelowChannelMin, NonceConflict…), сеть, HTTP.
    pub async fn send(
        &self,
        ctx: &Ctx,
        txs: &[Bytes],
        max_block: u64,
        terms: Terms<'_>,
        started_at: &mut Option<chrono::DateTime<chrono::Utc>>,
    ) -> GatewayResult<()> {
        let cond = match (self.cond, terms.head_time) {
            (Some(w), Some(t)) => Cond::Until(t.saturating_add(w)),
            _ => Cond::Plain,
        };
        self.post(ctx, txs, max_block, (cond, terms.known), Some(started_at))
            .await
    }

    /// «Проверить подключение» (send/check.rs): пустая транзакция тем методом, которым канал отправляет; у канала с
    /// окном (решение 436) — условный метод с пустыми условиями. Ошибки: как send.
    pub async fn probe(&self, ctx: &Ctx) -> GatewayResult<()> {
        let cond = if self.cond.is_some() {
            Cond::Probe
        } else {
            Cond::Plain
        };
        super::super::node::chain_seen::invalidate(ctx, &self.url);
        self.post(ctx, &[Bytes::new()], 0, (cond, None), None).await
    }

    /// Запрос в канал и разбор ответа. Принимает: транзакции, предел блока, условие и knownAccounts пулов (решение
    /// 438). Ошибки: как send; у условной отправки — с пояснением кода секвенсора (send/sequencer_cond.rs explain).
    async fn post(
        &self,
        ctx: &Ctx,
        txs: &[Bytes],
        max_block: u64,
        (cond, known): (Cond, Option<&Map<String, Value>>),
        started_at: Option<&mut Option<chrono::DateTime<chrono::Utc>>>,
    ) -> GatewayResult<()> {
        // Проверяется именно endpoint отправки: channel.url может отличаться от узла рыночных данных.
        super::super::node::chain_seen::verify_http(ctx, &self.url).await?;
        if self.endpoint_config.is_some_and(|v| {
            v != (
                ctx.settings.generation(),
                crate::secrets::vault::generation(),
            )
        }) {
            return Err(ctx.err(
                crate::gateway::errors::ErrorCode::NotConfigured,
                "Настройки или ключ изменились после подготовки канала отправки",
            ));
        }
        let seq = self.ch.single == Some(Single::Sequencer);
        let conditional = seq && cond != Cond::Plain;
        let sync = seq && !conditional && matches!(self.submit, Submit::Sync(_));
        let body = self.body(txs, max_block, cond, known);
        let auth;
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let (Auth::Header, Some(k)) = (self.ch.auth, &self.key) {
            auth = k.clone();
            headers.push(("Authorization", auth.as_str()));
        }
        let lim = self.http_ms;
        let stamp = super::super::node::chain_seen::watch(ctx, &self.url);
        let probe = started_at.is_none();
        let result = async {
            let text = rpc::post_within(
                ctx,
                &self.url,
                &body,
                &headers,
                self.ch.title,
                lim,
                started_at,
            )
            .await?;
            match jsonrpc::parse_text(&text, self.ch.title, Op::Send) {
                Ok(raw) if crate::settings::evm_profiles::profile(ctx.place.id).is_some() => {
                    let hash: alloy_primitives::B256 = jsonrpc::decode(&raw, self.ch.title)?;
                    if hash != alloy_primitives::keccak256(&txs[0]) {
                        return Err(ctx.err(
                            crate::gateway::errors::ErrorCode::BadResponse,
                            "eth_sendRawTransaction вернул хеш другой транзакции",
                        ));
                    }
                    Ok(())
                }
                Ok(_) => Ok(()),
                Err(e) if e.message.to_ascii_lowercase().contains("already known") => Ok(()),
                Err(e) if conditional => Err(sequencer_cond::explain(&text, e)),
                Err(e) if sync => Err(seq_opts::explain_sync(&text, e)),
                Err(e) => Err(e),
            }
        }
        .await;
        result.inspect_err(|e| {
            if !probe || super::super::node::failover::switchable(e) {
                super::super::node::chain_seen::failed(ctx, stamp);
            }
        })
    }

    /// Тело запроса в формате канала (условие и eth_sendRawTransactionSync — только у Single::Sequencer).
    fn body(
        &self,
        txs: &[Bytes],
        max_block: u64,
        cond: Cond,
        known: Option<&Map<String, Value>>,
    ) -> Value {
        let no0x = |b: &Bytes| alloy_primitives::hex::encode(b);
        let (method, params) = match (self.method, self.ch.single, self.ch.bundle) {
            (SendMethod::Single, Some(Single::Raw), _) => {
                ("eth_sendRawTransaction", json!([txs[0]]))
            }
            (SendMethod::Single, Some(Single::Sequencer), _) if cond != Cond::Plain => (
                sequencer_cond::METHOD,
                sequencer_cond::params(&txs[0], cond, known),
            ),
            (SendMethod::Single, Some(Single::Sequencer), _) => match self.submit {
                Submit::Sync(t) => (seq_opts::SYNC_METHOD, seq_opts::sync_params(&txs[0], t)),
                Submit::Raw => ("eth_sendRawTransaction", json!([txs[0]])),
            },
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
