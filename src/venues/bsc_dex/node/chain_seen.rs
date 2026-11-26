//! Файл: venues/bsc_dex/node/chain_seen.rs
//! Что делает: решение 416 (ревью шага 1, М-1) — подтверждённая сеть узлов HTTP (основного и резервного) в окружении
//! адаптера (Ctx.chains) и сверка перед подписью: транзакция подписывается chainId из dex.evm_chain_id, только если
//! eth_chainId каждого заданного узла ему равен. Подтверждение хранится по адресу узла (значение настройки, без ключа)
//! вместе с dex.evm_chain_id на момент сверки: сменили адрес узла или dex.evm_chain_id на лету — подтверждения нет,
//! сверка заново (один eth_chainId на узел). Узел другой сети — отказ с точной причиной (до отправки, а не «все каналы
//! отказали»). Основной узел не ответил — отказ (без подтверждённой сети не подписываем); резервный не ответил — не
//! мешает (подтверждение придёт позже), его неверная сеть — отказ.
//! Кто вызывает: exec/txs.rs (send_capped), exec/bundle_send.rs (send) — до отправки рядом с SendCfg::read (решение 398);
//! connection_extra.rs (резервный узел — запоминает ответ «Проверить подключение»).
//! От чего зависит: ctx.rs (node_url), node/rpc.rs (post), node/jsonrpc.rs, node/failover.rs (имена настроек узлов),
//! node/chain_id.rs.
//! Настройки: dex.evm_chain_id [bsc_dex]; bsc.node_http_url, bsc.node_http_url_backup (вся система + площадка DEX, решение 435).
//!
//! eth_chainId — https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth .

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::U64;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::errors::Op;
use super::failover::{Link, Node};
use super::{chain_id, jsonrpc, rpc};

/// Адрес узла (настройка, без ключа) → (chainId узла, dex.evm_chain_id при сверке); тикер газа (dex.gas_ticker),
/// прочитанный до последней отправки (gas_ticker.rs, решение 398). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Seen {
    nodes: Arc<Mutex<HashMap<String, (u64, u64)>>>,
    gas: Arc<Mutex<Option<String>>>,
    /// Проверенные HTTP-точки новых сетей, включая резерв и список узлов квитанций.
    /// Ключ охватывает эффективный URL (включая смену секрета), в памяти остаётся только его хеш.
    verified: super::endpoint_chain::Endpoints,
}

impl Seen {
    /// Что делает: запоминает тикер газа, прочитанный из настроек. Принимает: тикер. Ошибок нет.
    pub fn note_gas(&self, t: &str) {
        *self.gas.lock().unwrap_or_else(|p| p.into_inner()) = Some(t.to_string());
    }

    /// Что делает: тикер газа, прочитанный последним (None — ещё не читался). Ошибок нет.
    pub fn gas(&self) -> Option<String> {
        self.gas.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Что делает: запоминает ответ узла. Принимает: адрес узла, chainId узла, dex.evm_chain_id. Ошибок нет.
    pub fn note(&self, url: &str, got: u64, want: u64) {
        self.nodes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(url.to_string(), (got, want));
    }

    /// Что делает: chainId узла, подтверждённый при том же dex.evm_chain_id. Принимает: адрес, dex.evm_chain_id.
    /// Возвращает: None — не подтверждён (новый адрес, сменилась настройка). Ошибок нет.
    pub fn get(&self, url: &str, want: u64) -> Option<u64> {
        self.nodes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(url)
            .filter(|(_, w)| *w == want)
            .map(|(g, _)| *g)
    }
}

/// Проверяет eth_chainId у того URL, которому непосредственно уйдёт запрос. Проверки разных URL независимы.
pub async fn verify_http(ctx: &Ctx, url: &str) -> GatewayResult<()> {
    if let Some(check) = begin(ctx, url, false) {
        let body = jsonrpc::request(1, "eth_chainId", &json!([]));
        observed(ctx, url, &body, rpc::SOURCE, Some(check)).await?;
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) type Check = (
    super::endpoint_chain::Endpoint,
    u64,
    super::endpoint_chain::Stamp,
);
fn begin(ctx: &Ctx, url: &str, refresh: bool) -> Option<Check> {
    let key = endpoint(ctx, url)?;
    let settings = ctx.settings.generation();
    let epoch = ctx.chains.verified.check(key, settings, refresh)?;
    Some((key, settings, epoch))
}
/// Запрошенный оператором/мониторингом eth_chainId обновляет подтверждение по фактически
/// использованному URL. Для чтения другого метода подтверждение нужно только на холодном пути.
pub(in crate::venues::bsc_dex) async fn node_post(
    ctx: &Ctx,
    url: &str,
    body: &serde_json::Value,
    source: &str,
) -> GatewayResult<String> {
    if body.get("method").and_then(|v| v.as_str()) == Some("eth_chainId") {
        observed(ctx, url, body, source, watch(ctx, url)).await
    } else {
        verify_http(ctx, url).await?;
        let stamp = watch(ctx, url);
        rpc::post(ctx, url, body, &[], source)
            .await
            .inspect_err(|_| failed(ctx, stamp))
    }
}
async fn observed(
    ctx: &Ctx,
    url: &str,
    body: &serde_json::Value,
    source: &str,
    check: Option<Check>,
) -> GatewayResult<String> {
    let exchange = async {
        let text = rpc::post(ctx, url, body, &[], source).await?;
        if let Some((key, settings, epoch)) = check {
            let raw = jsonrpc::parse_text(&text, source, Op::Read)?;
            let got: U64 = jsonrpc::decode(&raw, source)?;
            chain_id::same_chain(ctx, got.to::<u64>())
                .map_err(|e| ctx.err(ErrorCode::BadResponse, e))?;
            if ctx.settings.generation() != settings
                || !ctx.chains.verified.confirm(key, settings, epoch)
            {
                return Err(ctx.err(
                    ErrorCode::BadResponse,
                    "Настройки или соединение изменились во время проверки сети HTTP",
                ));
            }
        }
        Ok(text)
    };
    exchange.await.inspect_err(|_| failed(ctx, check))
}
fn endpoint(ctx: &Ctx, url: &str) -> Option<super::endpoint_chain::Endpoint> {
    let expected = crate::settings::evm_profiles::profile(ctx.place.id)?.evm_chain_id;
    Some((alloy_primitives::keccak256(url.as_bytes()), expected))
}
/// Состояние проверки именно в момент начала запроса.
pub(in crate::venues::bsc_dex) fn watch(ctx: &Ctx, url: &str) -> Option<Check> {
    begin(ctx, url, true)
}
/// Ошибка задержавшегося запроса не снимает более свежую проверку этого адреса.
pub(in crate::venues::bsc_dex) fn failed(ctx: &Ctx, check: Option<Check>) {
    if let Some((key, settings, stamp)) = check {
        ctx.chains.verified.failed(key, settings, stamp)
    }
}
/// Следующая операция на этом endpoint вновь читает фактический eth_chainId.
pub fn invalidate(ctx: &Ctx, url: &str) {
    if let Some(k) = endpoint(ctx, url) {
        ctx.chains.verified.forget(k);
    }
}

/// Что делает: вердикт по узлу. Принимает: кто (основной/резервный), адрес, chainId узла, dex.evm_chain_id.
/// Возвращает: Ok или точная причина отказа подписи. Ошибок нет.
pub fn verdict(who: &str, url: &str, got: u64, want: u64) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{who} узел {url} — сеть chainId {got}, у площадки {want} (dex.evm_chain_id): транзакция не подписана"
        ))
    }
}

/// Что делает: eth_chainId узла по его адресу напрямую (мимо переключения). Принимает: окружение, узел.
/// Возвращает: (адрес-настройка, chainId) или None — узел не задан (резервный). Ошибки: связь, ответ, ключ узла.
async fn ask(ctx: &Ctx, node: Node) -> GatewayResult<Option<(String, u64)>> {
    let (url, key) = Link::Http.names(node);
    let Ok(plain) = ctx.settings.text(url, ctx.at()) else {
        return match node {
            Node::Main => Err(ctx.not_set(
                url,
                format!("Не задана настройка {url} — задайте её в интерфейсе"),
            )),
            Node::Backup => Ok(None),
        };
    };
    let full = ctx.node_url(url, key)?;
    let body = jsonrpc::request(1, "eth_chainId", &json!([]));
    let text = node_post(ctx, &full, &body, rpc::SOURCE).await?;
    let raw = jsonrpc::parse_text(&text, rpc::SOURCE, Op::Read)?;
    let got: U64 = jsonrpc::decode(&raw, rpc::SOURCE)?;
    Ok(Some((plain, got.to::<u64>())))
}

/// Что делает: сверка перед подписью (шапка). Принимает: окружение. Возвращает: chainId для подписи.
/// Ошибки: NotConfigured (dex.gas_ticker, dex.evm_chain_id, адрес основного узла), BadResponse (узел другой сети),
/// связь с основным.
pub async fn ensure(ctx: &Ctx) -> GatewayResult<u64> {
    // Решение 416 (ревью): тикер газа — до отправки (затраты газа после неё берут его из памяти, gas_ticker.rs sent).
    super::super::gas_ticker::ticker(ctx)?;
    let want = chain_id::chain_id(ctx)?;
    for (node, who) in [(Node::Main, "основной"), (Node::Backup, "резервный")] {
        let url = Link::Http.names(node).0;
        if let Ok(plain) = ctx.settings.text(url, ctx.at())
            && let Some(got) = ctx.chains.get(&plain, want)
        {
            verdict(who, &plain, got, want).map_err(|m| ctx.err(ErrorCode::BadResponse, m))?;
            continue;
        }
        match ask(ctx, node).await {
            Ok(Some((plain, got))) => {
                ctx.chains.note(&plain, got, want);
                verdict(who, &plain, got, want).map_err(|m| ctx.err(ErrorCode::BadResponse, m))?;
            }
            Ok(None) => {}
            Err(e) if node == Node::Main => {
                return Err(ctx.err(
                    e.code,
                    format!(
                        "сеть основного узла не подтверждена (eth_chainId): {} — транзакция не подписана",
                        e.message
                    ),
                ));
            }
            // Резервный не ответил — подписи не мешает (его сеть сверится при следующей подписи).
            Err(_) => {}
        }
    }
    Ok(want)
}
