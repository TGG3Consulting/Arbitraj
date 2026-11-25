//! The exchange rejected this exact API account and destination. A saved address is
//! insufficient evidence: Gate documents ADDRESS_NOT_USED as never used in the web console.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};

use super::{
    ctx::{Ctx, SECRET_API_KEY},
    errors::Op,
    num, rest,
};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    account: String,
    asset: String,
    network: String,
    address: String,
}

impl Key {
    fn new(ctx: &Ctx, asset: &str, network: &str, address: &str) -> GatewayResult<Self> {
        let key = ctx.vault.get(SECRET_API_KEY).map_err(|e| ctx.tag(e))?;
        // Gate API keys are high-entropy credentials. This digest survives restart without persisting the key.
        let account = hex::encode(Sha256::digest(key.as_bytes()));
        let address = if address.len() == 42
            && address.starts_with("0x")
            && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            address.to_ascii_lowercase()
        } else {
            address.to_owned()
        };
        Ok(Self {
            account,
            asset: asset.to_owned(),
            network: network.to_owned(),
            address,
        })
    }
}

#[derive(Default)]
pub(super) struct Destinations {
    records: OnceCell<Mutex<HashMap<Key, State>>>,
    sequence: AtomicU64,
}

#[derive(Default, Copy, Clone)]
struct State {
    order: u64,
    rejected_at: Option<i64>,
}

pub(super) struct Attempt {
    key: Key,
    sequence: u64,
    epoch: u64,
    started: i64,
}

impl Destinations {
    async fn records(&self, ctx: &Ctx) -> GatewayResult<&Mutex<HashMap<Key, State>>> {
        self.records.get_or_try_init(|| async {
            let rows = ctx.journal.db().ledger().read(|c| {
                let mut q=c.prepare("SELECT account,asset,network,address,rejected_at FROM gate_withdraw_denial")?;
                q.query_map([], |r| Ok((Key {account:r.get(0)?,asset:r.get(1)?,network:r.get(2)?,address:r.get(3)?},
                    State{order:0,rejected_at:Some(r.get(4)?)})))?.collect::<rusqlite::Result<HashMap<_,_>>>()
            }).await.map_err(|e| internal(ctx,e))?;
            Ok(Mutex::new(rows))
        }).await
    }

    pub(super) async fn check(
        &self,
        ctx: &Ctx,
        asset: &str,
        network: &str,
        address: &str,
    ) -> GatewayResult<()> {
        let states = self.records(ctx).await?.lock().await;
        if states.is_empty() {
            return Ok(());
        }
        let key = Key::new(ctx, asset, network, address)?;
        if states.get(&key).is_some_and(|r| r.rejected_at.is_some()) {
            return Err(ctx.tag(GatewayError::new(ErrorCode::InvalidAddress,None,format!(
                "Gate отклонила вывод {asset} в сети {network} на {address} (ADDRESS_NOT_USED). После успешного вывода из кабинета нажмите «Проверить подключение» Gate."))));
        }
        Ok(())
    }

    pub(super) async fn attempt(
        &self,
        ctx: &Ctx,
        asset: &str,
        network: &str,
        address: &str,
    ) -> GatewayResult<Attempt> {
        self.records(ctx).await?;
        let epoch = crate::gateway::addr_book::key_epoch();
        let key = Key::new(ctx, asset, network, address)?;
        Ok(Attempt {
            key,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            epoch,
            started: Utc::now().timestamp(),
        })
    }

    pub(super) async fn record(&self, ctx: &Ctx, a: &Attempt, denied: bool) -> GatewayResult<()> {
        if a.epoch != crate::gateway::addr_book::key_epoch() {
            return Ok(());
        }
        let mut rows = self.records(ctx).await?.lock().await;
        let old = rows.get(&a.key).copied().unwrap_or_default();
        if a.sequence < old.order {
            return Ok(());
        }
        let rejected_at = denied.then_some(a.started);
        rows.insert(
            a.key.clone(),
            State {
                order: a.sequence,
                rejected_at,
            },
        );
        if old.rejected_at == rejected_at {
            return Ok(());
        }
        let key = a.key.clone();
        ctx.journal.db().ledger().write(move|c|{
            if let Some(at)=rejected_at{
                c.execute("INSERT INTO gate_withdraw_denial VALUES(?1,?2,?3,?4,?5)
                    ON CONFLICT(account,asset,network,address) DO UPDATE SET rejected_at=excluded.rejected_at",
                    rusqlite::params![key.account,key.asset,key.network,key.address,at])?;
            }else{
                c.execute("DELETE FROM gate_withdraw_denial WHERE account=?1 AND asset=?2 AND network=?3 AND address=?4",
                    rusqlite::params![key.account,key.asset,key.network,key.address])?;
            }
            Ok(())
        }).await.map_err(|e| internal(ctx,e))
    }

    pub(super) async fn reconcile(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let epoch = crate::gateway::addr_book::key_epoch();
        let rows = self.records(ctx).await?.lock().await;
        if rows.values().all(|s| s.rejected_at.is_none()) {
            return Ok(0);
        }
        let account = Key::new(ctx, "", "", "")?.account;
        let blocked: Vec<_> = rows
            .iter()
            .filter(|(k, _)| k.account == account)
            .filter_map(|(k, s)| Some((k.clone(), s.rejected_at?, s.order)))
            .collect();
        drop(rows);
        let mut cleared = 0;
        for (key, at, order) in blocked {
            let mut offset = 0;
            loop {
                let p = vec![
                    ("currency", key.asset.clone()),
                    ("from", at.to_string()),
                    ("limit", "100".into()),
                    ("offset", offset.to_string()),
                ];
                let list =
                    rest::signed(ctx, Method::GET, "/wallet/withdrawals", p, None, Op::Other)
                        .await?;
                let list = list
                    .as_array()
                    .ok_or_else(|| super::errors::bad("История выводов Gate не массив", ""))?;
                if list.iter().any(|r| done_after(r, &key, at)) {
                    // Only clear the exact refusal that this query observed. A later answer wins.
                    if epoch == crate::gateway::addr_book::key_epoch() {
                        let proof = Attempt {
                            key: key.clone(),
                            sequence: order,
                            epoch,
                            started: at,
                        };
                        self.record(ctx, &proof, false).await?;
                        cleared += 1;
                    }
                    break;
                }
                if list.len() < 100 {
                    break;
                }
                offset += 100;
            }
        }
        Ok(cleared)
    }
}

fn done_after(row: &Value, key: &Key, at: i64) -> bool {
    let Some(address) = num::text_opt(row, "address") else {
        return false;
    };
    let addr_match = address == key.address
        || (key.address.starts_with("0x")
            && key.address.len() == 42
            && key.address.eq_ignore_ascii_case(&address));
    row["status"] == "DONE"
        && row["currency"] == key.asset
        && row["chain"] == key.network
        && addr_match
        && num::text_opt(row, "memo").unwrap_or_default().is_empty()
        && num::text_opt(row, "timestamp")
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|started| started > at)
        && num::text_opt(row, "txid").is_some_and(|h| !h.is_empty())
}

fn internal(ctx: &Ctx, e: crate::storage::db::DbError) -> GatewayError {
    ctx.tag(GatewayError::new(
        ErrorCode::Internal,
        None,
        format!("Учёт отказа адреса вывода Gate: {e}"),
    ))
}
