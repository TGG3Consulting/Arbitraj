//! Переносимое taker-поручение доступно только выбранному локальному узлу. Здесь нет общего RPC, резервов и сетевого журнала.
use super::wire;
use crate::{
    gateway::{
        errors::{ErrorCode, GatewayResult},
        http::{self, Limits},
    },
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, TxKind, U64, U256, keccak256};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    net::IpAddr,
    path::{Component, PathBuf},
    sync::OnceLock,
};

/// Результат последней личной симуляции непосредственно перед подписанием внешней транзакции.
#[derive(Clone, Copy)]
pub(in crate::venues::bsc_dex) struct Measurement {
    pub(super) owner: Address,
    pub(super) chain: u64,
    pub(super) target: Address,
    pub(super) digest: B256,
    pub(super) value: U256,
    pub(super) gas: u64,
}

impl Measurement {
    fn matches(&self, chain: u64, from: Address, to: TxKind, data: &Bytes, value: U256) -> bool {
        self.chain == chain
            && to == TxKind::Call(self.target)
            && from == self.owner
            && value == self.value
            && data.get(..4) == Some(wire::abi::toPMMCall::SELECTOR.as_slice())
            && self.digest == keccak256(data)
            && self.gas > 0
    }

    fn affordable(price: U256, limit: u64, budget: U256) -> bool {
        price
            .checked_mul(U256::from(limit))
            .is_some_and(|max| max <= budget)
    }

    pub(in crate::venues::bsc_dex) fn gas_limit(
        &self,
        ctx: &Ctx,
        chain: u64,
        from: Address,
        call: (TxKind, &Bytes, U256),
        price: U256,
    ) -> GatewayResult<u64> {
        use crate::venues::bsc_dex::{
            num,
            send::{
                gas_mode::{self, GasMode},
                tx,
            },
        };
        let denied = || {
            ctx.err(
                ErrorCode::NotApplicable,
                "Tokenlon: сеть или поручение изменились после личной симуляции",
            )
        };
        if gas_mode::read(ctx)? != GasMode::Ethereum
            || ctx.chain_id() != Some(chain)
            || !self.matches(chain, from, call.0, call.1, call.2)
        {
            return Err(denied());
        }
        let limit = tx::with_margin(
            ctx,
            U64::from(self.gas),
            ctx.dec("send.gas_limit_margin_pct")?,
        )?;
        let budget =
            num::to_units(ctx.dec("tokenlon.gas_budget_native")?, 18).ok_or_else(denied)?;
        if !Self::affordable(price, limit, budget) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Tokenlon: перед отправкой сетевой расход превысил лимит кошелька",
            ));
        }
        Ok(limit)
    }
}

enum Endpoint {
    Ipc(PathBuf),
    Http(Url),
}
fn endpoint(raw: &str) -> Option<Endpoint> {
    if raw.starts_with('/') {
        let path = PathBuf::from(raw);
        return (!raw.starts_with("//")
            && !raw.contains('\0')
            && !raw.contains('\\')
            && path.is_absolute()
            && path.file_name().is_some()
            && !path.components().any(|c| matches!(c, Component::ParentDir)))
        .then_some(Endpoint::Ipc(path));
    }
    let u = Url::parse(raw).ok()?;
    let host = u
        .host_str()?
        .trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .ok()?;
    (u.scheme() == "http"
        && host.is_loopback()
        && u.username().is_empty()
        && u.password().is_none()
        && u.port_or_known_default().is_some()
        && u.query().is_none()
        && u.fragment().is_none())
    .then_some(Endpoint::Http(u))
}
pub(super) fn configured(ctx: &Ctx) -> bool {
    ctx.text("tokenlon.local_rpc")
        .ok()
        .and_then(|s| endpoint(&s))
        .is_some()
}

#[cfg(unix)]
async fn ipc(path: PathBuf, data: &Value, lim: Limits) -> Option<String> {
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
    };
    let mut socket = tokio::time::timeout(lim.head, UnixStream::connect(path))
        .await
        .ok()?
        .ok()?;
    let mut bytes = serde_json::to_vec(data).ok()?;
    bytes.push(b'\n');
    tokio::time::timeout(lim.idle, socket.write_all(&bytes))
        .await
        .ok()?
        .ok()?;
    let mut reply = Vec::new();
    let max = u64::try_from(lim.max_bytes).ok()?.checked_add(1)?;
    let mut limited = BufReader::new(socket.take(max));
    tokio::time::timeout(lim.idle, limited.read_until(b'\n', &mut reply))
        .await
        .ok()?
        .ok()?;
    if reply.len() > lim.max_bytes || reply.last() != Some(&b'\n') {
        return None;
    }
    String::from_utf8(reply).ok()
}
#[cfg(not(unix))]
async fn ipc(_: PathBuf, _: &Value, _: Limits) -> Option<String> {
    None
}

async fn exchange(endpoint: Endpoint, body: Value, lim: Limits) -> Option<Value> {
    let text = match endpoint {
        Endpoint::Ipc(path) => tokio::time::timeout(lim.total, ipc(path, &body, lim))
            .await
            .ok()??,
        Endpoint::Http(url) => {
            static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
            let client = CLIENT
                .get_or_init(|| {
                    Client::builder()
                        .http1_only()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .ok()
                })
                .as_ref()?;
            let (status, text) = http::exchange(client.post(url).json(&body), lim)
                .await
                .ok()?;
            if !status.is_success() {
                return None;
            }
            text
        }
    };
    let mut v: Value = serde_json::from_str(&text).ok()?;
    if v["jsonrpc"] != "2.0" || v["id"] != 1 || !v["error"].is_null() {
        return None;
    }
    Some(v.get_mut("result")?.take())
}
pub(super) async fn simulate<T: DeserializeOwned>(ctx: &Ctx, params: Value) -> GatewayResult<T> {
    let target=ctx.text("tokenlon.local_rpc").ok().and_then(|s|endpoint(&s)).ok_or_else(||
        ctx.err(ErrorCode::NotConfigured,"Для автоторговли Tokenlon укажите собственный Ethereum IPC или числовой loopback HTTP в tokenlon.local_rpc"))?;
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Собственный Ethereum-узел не подтвердил локальную симуляцию Tokenlon",
        )
    };
    let result = exchange(
        target,
        json!({"jsonrpc":"2.0","id":1,"method":"eth_simulateV1","params":params}),
        ctx.limits()?,
    )
    .await
    .ok_or_else(bad)?;
    serde_json::from_value(result).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn private_gas_stays_with_the_measured_archive_call_and_budget() {
        let archive:Value=serde_json::from_reader(flate2::read::GzDecoder::new(
            &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz")[..])).unwrap();
        let row = &archive["rows"][0];
        let tx = &row["tx"];
        let owner = tx["from"].as_str().unwrap().parse().unwrap();
        let target = tx["to"].as_str().unwrap().parse().unwrap();
        let data: Bytes = tx["input"].as_str().unwrap().parse().unwrap();
        let chain: U64 = serde_json::from_value(tx["chainId"].clone()).unwrap();
        let value: U256 = serde_json::from_value(tx["value"].clone()).unwrap();
        let chain = chain.to::<u64>();
        let gas = row["gasUsed"].as_u64().unwrap();
        let price = U256::from(row["effectiveGasPrice"].as_u64().unwrap());
        let measured = Measurement {
            owner,
            target,
            value,
            chain,
            gas,
            digest: keccak256(&data),
        };
        assert!(measured.matches(chain, owner, TxKind::Call(target), &data, value));
        assert!(!measured.matches(chain + 1, owner, TxKind::Call(target), &data, value));
        assert!(!measured.matches(chain, owner, TxKind::Call(owner), &data, value));
        assert!(!measured.matches(chain, owner, TxKind::Call(target), &data.slice(..4), value));
        let bill = price * U256::from(gas);
        assert!(Measurement::affordable(price, gas, bill));
        assert!(!Measurement::affordable(price, gas, bill - U256::from(1)));
        assert!(!Measurement::affordable(U256::MAX, gas, U256::MAX));
    }
    #[test]
    fn transferable_payload_is_only_addressable_to_an_explicit_local_socket() {
        assert!(matches!(
            endpoint("http://127.0.0.1:8545"),
            Some(Endpoint::Http(_))
        ));
        assert!(matches!(
            endpoint("http://[::1]:8545"),
            Some(Endpoint::Http(_))
        ));
        #[cfg(unix)]
        assert!(matches!(
            endpoint("/var/lib/geth/geth.ipc"),
            Some(Endpoint::Ipc(_))
        ));
        for s in [
            "",
            "geth.ipc",
            "//outside/geth.ipc",
            "/tmp/../geth.ipc",
            "http://localhost:8545",
            "http://127.0.0.1.example",
            "https://eth.llamarpc.com",
            "http://8.8.8.8:8545",
            "http://user@127.0.0.1:8545",
            "http://127.0.0.1:8545/?goto=x",
        ] {
            assert!(endpoint(s).is_none(), "{s}");
        }
    }

    #[tokio::test]
    async fn a_loopback_redirect_never_reaches_the_second_listener() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let original = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", original.local_addr().unwrap())).unwrap();
        let location = other.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut conn, _) = original.accept().await.unwrap();
            let mut buf = [0; 4096];
            let _ = conn.read(&mut buf).await.unwrap();
            let wire = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{location}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            conn.write_all(wire.as_bytes()).await.unwrap();
        });
        let lim = Limits {
            head: Duration::from_secs(2),
            idle: Duration::from_secs(2),
            total: Duration::from_secs(2),
            max_bytes: 4096,
        };
        assert!(
            exchange(
                Endpoint::Http(url),
                json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}),
                lim
            )
            .await
            .is_none()
        );
        server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), other.accept())
                .await
                .is_err()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn geth_ipc_uses_newline_framing_and_checks_request_id() {
        use tokio::{
            io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
            net::UnixListener,
        };
        let mut name = [0u8; 12];
        getrandom::fill(&mut name).unwrap();
        let path = std::env::temp_dir().join(format!(
            "tokenlon-{}.ipc",
            alloy_primitives::hex::encode(name)
        ));
        let server = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            for id in [2, 1] {
                let (mut s, _) = server.accept().await.unwrap();
                let mut line = String::new();
                BufReader::new(&mut s).read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(req["method"], "eth_simulateV1");
                assert_eq!(req["id"], 1);
                let response = format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"result":[{"number":"0x1"}]})
                );
                s.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let req = json!({"jsonrpc":"2.0","id":1,"method":"eth_simulateV1","params":[]});
        let lim = Limits {
            head: Duration::from_secs(1),
            idle: Duration::from_secs(1),
            total: Duration::from_secs(2),
            max_bytes: 4096,
        };
        assert!(
            exchange(Endpoint::Ipc(path.clone()), req.clone(), lim)
                .await
                .is_none()
        );
        assert_eq!(
            exchange(Endpoint::Ipc(path.clone()), req, lim)
                .await
                .unwrap(),
            json!([{"number":"0x1"}])
        );
        task.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
