//! Источник только перечисляет orderBytes. Торговые поля подтверждает сама книга вызовом quote от модуля.
use super::{profile::Source, wire::Order, *};
use crate::gateway::http;
use alloy::sol_types::SolEvent;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
type Raw = HashMap<B256, Bytes>;
const QUERY: &str = "query($h:Bytes!,$id:Bytes!){orders(first:250,orderBy:id,orderDirection:asc,where:{active:true,id_gt:$id},block:{hash:$h}){id orderHash orderBytes raindex{id}}}";
async fn page(ctx: &Ctx, r: reqwest::RequestBuilder) -> Option<Value> {
    let (status, body) = http::exchange(r, ctx.limits().ok()?).await.ok()?;
    status
        .is_success()
        .then(|| serde_json::from_str(&body).ok())
        .flatten()
}
impl Source {
    async fn indexed(&self, ctx: &Ctx, h: white_state::Head) -> Option<Raw> {
        let meta = page(
            ctx,
            ctx.http
                .post(self.url.clone())
                .json(&json!({"query":"{_meta{block{number hash}}}"})),
        )
        .await?;
        let indexed = &meta["data"]["_meta"]["block"];
        let n = indexed["number"].as_u64()?;
        let anchor = if n >= h.number.to::<u64>() {
            h.hash
        } else {
            // Источник может отставать. Достаточность каждой заявки повторно проверяется самой книгой на h.
            let indexed_hash: B256 = indexed["hash"].as_str()?.parse().ok()?;
            let block = super::white_state::head(ctx, &format!("0x{n:x}"))
                .await
                .ok()?;
            if indexed_hash != block.hash
                || h.timestamp
                    .to::<u64>()
                    .saturating_sub(block.timestamp.to::<u64>())
                    .saturating_mul(1000)
                    > ctx.int("raindex.max_age_ms").ok()?
            {
                return None;
            }
            block.hash
        };
        let mut last = String::from("0x");
        let mut rows = Raw::new();
        loop {
            let j = page(
                ctx,
                ctx.http
                    .post(self.url.clone())
                    .json(&json!({"query":QUERY,"variables":{"h":anchor,"id":last}})),
            )
            .await?;
            let batch = j["data"]["orders"].as_array()?;
            for r in batch {
                let cursor = r["id"].as_str()?;
                if cursor <= last.as_str() {
                    return None;
                }
                last = cursor.to_owned();
                if r["raindex"]["id"].as_str()?.parse::<Address>().ok()? == self.profile.book {
                    rows.insert(
                        r["orderHash"].as_str()?.parse().ok()?,
                        r["orderBytes"].as_str()?.parse().ok()?,
                    );
                }
            }
            if batch.len() < 250 {
                return Some(rows);
            }
        }
    }
    async fn history(&self, ctx: &Ctx, h: white_state::Head) -> Option<Raw> {
        let event = match self.profile.version {
            3 => wire::AddOrder::SIGNATURE_HASH,
            4 => wire::AddOrderV2::SIGNATURE_HASH,
            _ => wire::AddOrderV3::SIGNATURE_HASH,
        };
        let mut rows = Raw::new();
        let mut cursor = None::<(u64, u64)>;
        let mut used = HashSet::new();
        loop {
            let mut url = self.url.clone();
            if let Some((block, index)) = cursor {
                url.query_pairs_mut()
                    .append_pair("block_number", &block.to_string())
                    .append_pair("index", &index.to_string());
            }
            let j = page(ctx, ctx.http.get(url)).await?;
            for r in j["items"].as_array()? {
                if r["block_number"].as_u64()? > h.number.to::<u64>()
                    || r["address"]["hash"].as_str()?.parse::<Address>().ok()? != self.profile.book
                    || r["topics"][0].as_str()?.parse::<B256>().ok()? != event
                {
                    continue;
                }
                if let Some((hash, blob)) = r["data"]
                    .as_str()
                    .and_then(|v| v.parse::<Bytes>().ok())
                    .and_then(|v| Order::added(self.profile.version, &v))
                {
                    rows.insert(hash, blob);
                }
            }
            if j["next_page_params"].is_null() {
                return Some(rows);
            }
            let v = &j["next_page_params"];
            let next = (v["block_number"].as_u64()?, v["index"].as_u64()?);
            if !used.insert(next) {
                return None;
            }
            cursor = Some(next);
        }
    }
    pub(super) async fn read(
        &self,
        p: &Parts,
        h: white_state::Head,
        quote: Address,
    ) -> Option<Vec<Live>> {
        self.profile.verify(&p.ctx, h).await.ok()?;
        let raw = if self.graphql {
            self.indexed(&p.ctx, h).await?
        } else {
            self.history(&p.ctx, h).await?
        };
        let known: HashSet<_> = p.tokens.all().into_iter().map(|v| v.address).collect();
        let bridges = cash::State::configured(&p.ctx, self.profile, h, quote).await;
        let mut legs = vec![];
        for (hash, data) in raw {
            let Some(order) = Order::decode(self.profile.version, &data, hash) else {
                continue;
            };
            let order = std::sync::Arc::new(order);
            let (a, b) = order.pairs();
            for (i, input) in a.iter().enumerate() {
                for (j, output) in b.iter().enumerate() {
                    if input == output {
                        continue;
                    }
                    let leg = Leg {
                        p: self.profile,
                        via: None,
                        order: order.clone(),
                        hash,
                        i,
                        j,
                        input: *input,
                        output: *output,
                    };
                    let eligible = |leg: &Leg| {
                        let (a, b) = leg.pair();
                        a != b
                            && ((a == quote && known.contains(&b))
                                || (b == quote && known.contains(&a)))
                    };
                    if eligible(&leg) {
                        legs.push(leg.clone());
                    }
                    for bridge in &bridges {
                        if *input != bridge.role.bridge && *output != bridge.role.bridge {
                            continue;
                        }
                        let candidate = Leg {
                            via: Some(*bridge),
                            ..leg.clone()
                        };
                        if eligible(&candidate) {
                            legs.push(candidate);
                        }
                    }
                }
            }
        }
        use futures_util::{StreamExt, stream};
        let groups = stream::iter(legs)
            .map(|leg| async move {
                if leg.p.version == 3 {
                    return legacy::read(p, leg, h).await;
                }
                match leg.current(&p.ctx, h).await {
                    Some((cap, ratio, _)) => vec![Live {
                        leg,
                        cap,
                        ratio,
                        max_budget: None,
                    }],
                    None => vec![],
                }
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        Some(groups.into_iter().flatten().collect())
    }
}
