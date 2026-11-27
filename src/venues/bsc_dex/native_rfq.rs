//! Native RFQ v6. Адреса и хеши привязаны к настройкам выбранной сети.
//! Общий ручной интерфейс возвращает только открытые условия и результат симуляции кошелька.
use super::{
    ctx::Ctx,
    node::{abi::IERC20, rpc},
    quote::white_state,
    wallet,
};
use crate::{
    gateway::{
        errors::{ErrorCode, GatewayResult},
        http,
        source_quote::{Preview, Request},
    },
    settings::scope::Scope,
};
use alloy::{rpc::types::Log, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U64, U256, keccak256};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(in crate::venues::bsc_dex) mod book;
pub(in crate::venues::bsc_dex) mod routing;
pub(in crate::venues::bsc_dex) mod settlement;
pub(in crate::venues::bsc_dex) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("native_rfq.trading").unwrap_or(false)
}
mod abi;
mod client;
mod wire;
pub(super) const LABEL: &str="Native RFQ v6";
const PROFILE: &str = "native_rfq.roles";

#[derive(Clone, Copy)]
struct Role {
    address: Address,
    hash: B256,
}
struct Profile {
    chain: u64,
    slug: String,
    router: Role,
    pool: Role,
    module: Option<Role>,
}
impl Profile {
    fn parse(rows: &Value) -> Option<Self> {
        let mut roles = BTreeMap::new();
        let mut network = None;
        for row in rows.as_array()? {
            let mut fields = row.as_str()?.split_whitespace();
            let name = fields.next()?;
            let first = fields.next()?;
            let second = fields.next()?;
            if fields.next().is_some() {
                return None;
            }
            if name == "network" {
                if network.is_some()
                    || second.is_empty()
                    || !second
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                {
                    return None;
                }
                network = Some((first.parse::<u64>().ok()?, second.to_owned()));
            } else {
                let role = Role {
                    address: first.parse().ok()?,
                    hash: second.parse().ok()?,
                };
                if role.address.is_zero()
                    || role.hash.is_zero()
                    || !matches!(name, "router" | "pool" | "module")
                    || roles.insert(name, role).is_some()
                {
                    return None;
                }
            }
        }
        let (chain, slug) = network?;
        let router = roles.remove("router")?;
        let pool = roles.remove("pool")?;
        let module = roles.remove("module");
        if chain == 0
            || router.address == pool.address
            || module.is_some_and(|m| m.address == pool.address || m.address == router.address)
        {
            return None;
        }
        Some(Self {
            chain,
            slug,
            router,
            pool,
            module,
        })
    }
    fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let scope = Scope::Venue(ctx.place.id.to_string());
        let rows = ctx.settings.get_exact(PROFILE, &scope);
        rows.as_ref().and_then(Self::parse).filter(|p|Some(p.chain)==ctx.chain_id())
          .ok_or_else(||ctx.not_set(PROFILE,"Укажите сеть, router и pool с хешами кода в настройках Native RFQ этой площадки"))
    }
    async fn verify(&self, ctx: &Ctx, head: white_state::Head) -> GatewayResult<()> {
        let verify = async |r: Role| {
            let code: Bytes =
                rpc::call(ctx, "eth_getCode", json!([r.address, head.selector()])).await?;
            if code.is_empty() || keccak256(&code) != r.hash {
                return Err(ctx.err(
                    ErrorCode::NoRoute,
                    "Изменился код одного из настроенных контрактов Native RFQ",
                ));
            }
            Ok(())
        };
        let optional = async {
            if let Some(m) = self.module {
                verify(m).await?;
                let read = async |method: &str| -> GatewayResult<B256> {
                    let selector = Bytes::copy_from_slice(&keccak256(method.as_bytes())[..4]);
                    let result: Bytes = rpc::call(
                        ctx,
                        "eth_call",
                        json!([{"to":m.address,"input":selector},head.selector()]),
                    )
                    .await?;
                    if result.len() != 32 {
                        return Err(bad(ctx, "Неполное подтверждение ролей модуля Native"));
                    }
                    Ok(B256::from_slice(&result))
                };
                let (router, pool, router_hash, pool_hash) = tokio::try_join!(
                    read("router()"),
                    read("pool()"),
                    read("routerHash()"),
                    read("poolHash()")
                )?;
                if router != self.router.address.into_word()
                    || pool != self.pool.address.into_word()
                    || router_hash != self.router.hash
                    || pool_hash != self.pool.hash
                {
                    return Err(bad(
                        ctx,
                        "Модуль Native привязан к другому Router или RFQPool",
                    ));
                }
            }
            Ok(())
        };
        tokio::try_join!(verify(self.router), verify(self.pool), optional)?;
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Child {
    parent_hash: B256,
    timestamp: U64,
    calls: Vec<Applied>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Applied {
    status: U64,
    return_data: Bytes,
    gas_used: U64,
    logs: Vec<Log>,
}

struct Checked {
    firm: wire::Firm,
    received: U256,
    gas: u64,
    head: white_state::Head,
}
fn bad(ctx: &Ctx, why: &str) -> crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::BadResponse, why)
}
fn balance(who: Address, token: Address) -> Value {
    let data = Bytes::from(IERC20::balanceOfCall { owner: who }.abi_encode());
    json!({"from":who,"to":token,"input":data})
}
async fn check(
    ctx: &Ctx,
    profile: &Profile,
    payer: Address,
    input: Address,
    output: Address,
    amount: U256,
) -> GatewayResult<Checked> {
    let generation = ctx.settings.generation();
    let mut url = client::url(ctx, "firm-quote")?;
    url.query_pairs_mut().extend_pairs([
        ("src_chain", profile.slug.clone()),
        ("dst_chain", profile.slug.clone()),
        ("token_in", input.to_string()),
        ("token_out", output.to_string()),
        ("from_address", payer.to_string()),
        ("beneficiary_address", payer.to_string()),
        ("amount_wei", amount.to_string()),
        ("version", "6".into()),
        ("allow_multihop", "false".into()),
    ]);
    let (status, body) = http::exchange(ctx.http.get(url), ctx.limits()?)
        .await
        .map_err(|_| bad(ctx, "Нет полного ответа Native RFQ"))?;
    if !status.is_success() {
        return Err(bad(ctx, "Native RFQ отклонил запрос"));
    }
    let response: Value =
        serde_json::from_str(&body).map_err(|_| bad(ctx, "Некорректный ответ Native RFQ"))?;
    let firm = wire::firm(
        &response,
        profile.router.address,
        profile.pool.address,
        payer,
        input,
        output,
        amount,
    )
    .ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "Нет одиночной Native RFQ v6 с подходящими условиями",
        )
    })?;
    let (received, gas, head) = simulate(ctx, profile, &firm, firm.proof.minimum).await?;
    if ctx.settings.generation() != generation {
        return Err(bad(ctx, "Изменился профиль Native RFQ"));
    }
    Ok(Checked {
        firm,
        received,
        gas,
        head,
    })
}
async fn simulate(
    ctx: &Ctx,
    profile: &Profile,
    firm: &wire::Firm,
    minimum: U256,
) -> GatewayResult<(U256, u64, white_state::Head)> {
    let generation = ctx.settings.generation();
    let proof = &firm.proof;
    let (payer, input, output, amount) = (proof.payer, proof.input, proof.output, proof.sold);
    let head = white_state::head(ctx, "latest").await?;
    profile.verify(ctx, head).await?;
    let latest = || Utc::now().timestamp().max(0) as u64;
    if firm.expiry <= latest() || firm.expiry <= head.timestamp.to::<u64>() {
        return Err(bad(ctx, "Предложение Native RFQ истекло"));
    }
    let (target, calldata) = match profile.module {
        Some(m) => (m.address, firm.call(minimum)),
        None => (profile.router.address, firm.raw.clone()),
    };
    let pay = balance(payer, input);
    let receive = balance(payer, output);
    let swap = json!({"from":payer,"to":target,"input":calldata});
    let children: Vec<Child> = rpc::call(
        ctx,
        "eth_simulateV1",
        json!([{"blockStateCalls":[{"calls":[pay,receive,swap,pay,receive]}]},head.selector()]),
    )
    .await?;
    let [child] = children.as_slice() else {
        return Err(bad(
            ctx,
            "Отсутствует единственный блок симуляции Native RFQ",
        ));
    };
    let [a, b, op, c, d] = child.calls.as_slice() else {
        return Err(bad(ctx, "Неполный набор расчётов Native RFQ"));
    };
    if child.parent_hash != head.hash
        || child.timestamp >= U64::from(firm.expiry)
        || firm.expiry <= latest()
        || child.calls.iter().any(|c| c.status != U64::from(1))
    {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Кошелёк не смог исполнить свежую Native RFQ на этом блоке",
        ));
    }
    let value =
        |v: &Applied| (v.return_data.len() == 32).then(|| U256::from_be_slice(&v.return_data));
    let balances = || {
        Some((
            value(a)?.checked_sub(value(c)?)?,
            value(d)?.checked_sub(value(b)?)?,
        ))
    };
    let (spent, received) =
        balances().ok_or_else(|| bad(ctx, "Нет полного снимка балансов Native RFQ"))?;
    if spent != amount
        || firm
            .proof
            .received(&op.logs, profile.module.map(|m| m.address))
            != Some(received)
        || ctx.settings.generation() != generation
    {
        return Err(bad(
            ctx,
            "Условия, событие или остатки Native RFQ расходятся",
        ));
    }
    if received < minimum {
        return Err(bad(ctx, "Не достигнут минимум владельца в Native RFQ"));
    }
    Ok((received, op.gas_used.to(), head))
}
pub(super) async fn preview(ctx: &Ctx, r: Request) -> GatewayResult<Preview> {
    let profile = Profile::read(ctx)?;
    let who = wallet::signer::address(ctx)?;
    if r.trader.is_some_and(|t| t != who)
        || r.input.is_zero()
        || r.output.is_zero()
        || r.input == r.output
        || r.market.is_some()
        || r.builder.is_some()
        || r.amount_tokens.is_some()
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для Native RFQ укажите два ERC20 и amount_raw для выбранного кошелька",
        ));
    }
    let amount = r
        .amount_raw
        .as_deref()
        .and_then(|s| U256::from_str_radix(s, 10).ok())
        .filter(|n| !n.is_zero())
        .ok_or_else(|| bad(ctx, "Нужна положительная сумма в минимальных единицах"))?;
    let q = check(ctx, &profile, who, r.input, r.output, amount).await?;
    Ok(Preview {
        source: "native_rfq".into(),
        input: r.input,
        output: r.output,
        spent_raw: amount.to_string(),
        received_raw: q.received.to_string(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: q.head.hash.to_string(),
        block: Some(q.head.number.to()),
        expires: Some(q.firm.expiry),
        details: json!({"market":profile.pool.address,"module":profile.module.map(|m|m.address),
        "quote_id":q.firm.proof.quote_id,"offer_digest":q.firm.proof.digest,"maker":q.firm.proof.maker,"gas":q.gas,"minimum":q.firm.proof.minimum.to_string()}),
    })
}
